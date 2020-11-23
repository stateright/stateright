use actix_web::{*, web::Json};
use crate::*;
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct StatusView {
    done: bool,
    model: String,
    generated: usize,
    discoveries: HashMap<&'static str, String>, // property name -> encoded path
}

#[derive(Debug, Eq, PartialEq)]
struct StateView<State, Action> {
    action: Option<Action>,
    outcome: Option<String>,
    state: State,
}

type StateViewsJson<State, Action> = Json<Vec<StateView<State, Action>>>;

impl<Action, State> serde::Serialize for StateView<State, Action>
where
    Action: Debug,
    State: Debug + Hash,
{
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let mut out = ser.serialize_struct("StateView", 3)?;
        if let Some(ref action) = self.action {
            out.serialize_field("action", &format!("{:?}", action))?;
        }
        if let Some(ref outcome) = self.outcome {
            out.serialize_field("outcome", outcome)?;
        }
        out.serialize_field("state", &format!("{:#?}", self.state))?;
        out.serialize_field("fingerprint", &format!("{:?}", fingerprint(&self.state)))?;
        out.end()
    }
}

pub(crate) fn serve<M, C>(checker: C, addresses: impl ToSocketAddrs) -> Arc<C>
where M: 'static + Model,
      M::Action: Debug,
      M::State: Debug + Hash,
      C: 'static + Checker<M> + Send + Sync,
{
    let checker = Arc::new(checker);

    let data = Arc::clone(&checker);
    HttpServer::new(move || {
        macro_rules! get_ui_file {
            ($filename:literal) => {
                web::get().to(|| HttpResponse::Ok().body({
                    if let Ok(content) = std::fs::read(concat!("./ui/", $filename)) {
                        log::info!("Explorer dev mode. Loading {} from disk.", $filename);
                        content
                    } else {
                        include_bytes!(concat!("../../ui/", $filename)).to_vec()
                    }
                }))
            }
        };

        App::new()
            .data(Arc::clone(&data))
            .route("/.status", web::get().to(status::<M, C>))
            .route("/.states{fingerprints:.*}", web::get().to(states::<M, C>))
            .route("/", get_ui_file!("index.htm"))
            .route("/app.css", get_ui_file!("app.css"))
            .route("/app.js", get_ui_file!("app.js"))
            .route("/knockout-3.5.0.js", get_ui_file!("knockout-3.5.0.js"))
    }).bind(addresses).unwrap().run().unwrap();

    checker
}

fn status<M, C>(_: HttpRequest, checker: web::Data<Arc<C>>) -> Result<Json<StatusView>>
where M: Model,
      M::State: Hash,
      C: Checker<M>,
{
    let status = StatusView {
        model: std::any::type_name::<M>().to_string(),
        done: checker.is_done(),
        generated: checker.generated_count(),
        discoveries: checker.discoveries().into_iter()
            .map(|(name, path)| (name, path.encode()))
            .collect(),
    };
    Ok(Json(status))
}

fn states<M, C>(req: HttpRequest, checker: web::Data<Arc<C>>) -> Result<StateViewsJson<M::State, M::Action>>
where M: Model,
      M::State: Debug + Hash,
      C: Checker<M>,
{
    let model = &checker.model();

    // extract fingerprints
    let mut fingerprints_str = req.match_info().get("fingerprints").expect("missing 'fingerprints' param").to_string();
    if fingerprints_str.ends_with('/') {
        let relevant_len = fingerprints_str.len() - 1;
        fingerprints_str.truncate(relevant_len);
    }
    let fingerprints: VecDeque<_> = fingerprints_str.split('/').filter_map(|fp| fp.parse::<Fingerprint>().ok()).collect();

    // ensure all but the first string (which is empty) were parsed
    if fingerprints.len() + 1 != fingerprints_str.split('/').count() {
        return Err(
            actix_web::error::ErrorNotFound(
                format!("Unable to parse fingerprints {}", fingerprints_str)));
    }

    // now build up all the subsequent `StateView`s
    let mut results = Vec::new();
    if fingerprints.is_empty() {
        for state in model.init_states() {
            results.push(StateView { action: None, outcome: None, state });
        }
    } else if let Some(last_state) = Path::final_state::<M>(model, fingerprints) {
        // Must generate the actions three times because they are consumed by `next_state`
        // and `display_outcome`.
        let mut actions1 = Vec::new();
        let mut actions2 = Vec::new();
        let mut actions3 = Vec::new();
        model.actions(&last_state, &mut actions1);
        model.actions(&last_state, &mut actions2);
        model.actions(&last_state, &mut actions3);
        for ((action, action2), action3) in actions1.into_iter().zip(actions2).zip(actions3) {
            let outcome = model.display_outcome(&last_state, action2);
            let state = model.next_state(&last_state, action3);
            if let Some(state) = state {
                results.push(StateView { action: Some(action), outcome, state });
            }
        }
    } else {
        return Err(
            actix_web::error::ErrorNotFound(
                format!("Unable to find state following fingerprints {}", fingerprints_str)));
    }

    Ok(Json(results))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::binary_clock::*;

    #[test]
    fn can_init() {
        let checker = Arc::new(BinaryClock.checker().spawn_bfs());
        assert_eq!(get_states(Arc::clone(&checker), "/").unwrap(), vec![
            StateView { action: None, outcome: None, state: 0 },
            StateView { action: None, outcome: None, state: 1 },
        ]);
    }

    #[test]
    fn can_next() {
        let checker = Arc::new(BinaryClock.checker().spawn_bfs());
        // We need a static string for TestRequest, so this is precomputed, but you can recompute
        // the values if needed as follows:
        // ```
        // let first = fingerprint(&1_i8);
        // let second = fingerprint(&0_i8);
        // let path_name = format!("/{}/{}", first, second);
        // println!("New path name is: {}", path_name);
        // ```
        assert_eq!(get_states(Arc::clone(&checker), "/2716592049047647680/9080728272894440685").unwrap(), vec![
            StateView { action: Some(BinaryClockAction::GoHigh), outcome: Some("1".to_string()), state: 1 },
        ]);
    }

    #[test]
    fn err_for_invalid_fingerprint() {
        let checker = Arc::new(BinaryClock.checker().spawn_bfs());
        assert_eq!(format!("{}", get_states(Arc::clone(&checker), "/one/two/three").unwrap_err()),
            "Unable to parse fingerprints /one/two/three");
        assert_eq!(format!("{}", get_states(Arc::clone(&checker), "/1/2/3").unwrap_err()),
            "Unable to find state following fingerprints /1/2/3");
    }

    #[test]
    fn smoke_test() {
        use crate::actor::{DuplicatingNetwork, Envelope, Id, LossyNetwork, System, SystemState};
        use crate::actor::actor_test_util::ping_pong::{PingPongCount, PingPongMsg::*, PingPongSystem};
        use crate::actor::SystemAction::*;
        use crate::util::HashableHashSet;
        use std::iter::FromIterator;

        let checker = Arc::new(PingPongSystem {
            max_nat: 2,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::No,
            maintains_history: true,
        }.into_model().checker().spawn_bfs());
        assert_eq!(
            get_states(Arc::clone(&checker), "/").unwrap(),
            vec![
                StateView {
                    action: None,
                    outcome: None,
                    state: SystemState {
                        actor_states: vec![Arc::new(PingPongCount(0)), Arc::new(PingPongCount(0))],
                        history: (0, 1),
                        is_timer_set: vec![],
                        network: HashableHashSet::from_iter(vec![
                            Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                        ]),
                    },
                },
            ]);
        // To regenerate the path if the fingerprint changes:
        // ```
        // let fp = fingerprint(&SystemState::<PingPongSystem> {
        //     actor_states: vec![Arc::new(PingPongCount(0)), Arc::new(PingPongCount(0))],
        //     history: (0, 1),
        //     is_timer_set: vec![],
        //     network: HashableHashSet::from_iter(vec![
        //         Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
        //     ]),
        // });
        // println!("New path name is: /{}", fp);
        // ```
        assert_eq!(
            get_states(Arc::clone(&checker), "/2298670378538534683").unwrap(),
            vec![
                StateView {
                    action: Some(Drop(Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),
                    outcome: None,
                    state: SystemState {
                        actor_states: vec![Arc::new(PingPongCount(0)), Arc::new(PingPongCount(0))],
                        history: (0, 1),
                        is_timer_set: vec![],
                        network: HashableHashSet::new(),
                    },
                },
                StateView {
                    action: Some(Deliver { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }),
                    outcome: Some("\
ActorStep {
    last_state: PingPongCount(
        0,
    ),
    next_state: Some(
        PingPongCount(
            1,
        ),
    ),
    commands: [
        Send(
            Id(0),
            Pong(
                0,
            ),
        ),
    ],
}".to_string()),
                    state: SystemState {
                        actor_states: vec![
                            Arc::new(PingPongCount(0)),
                            Arc::new(PingPongCount(1)),
                        ],
                        history: (1, 2),
                        is_timer_set: vec![],
                        network: HashableHashSet::from_iter(vec![
                            Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                        ]),
                    }
                }
            ]);
    }

    fn get_states<M, C>(checker: Arc<C>, path_name: &'static str)
                -> Result<Vec<StateView<M::State, M::Action>>>
    where M: Model,
          M::State: Debug + Hash,
          C: Checker<M>,
    {
        //use actix_web::test::*;
        let req = actix_web::test::TestRequest::get()
            .param("fingerprints", &path_name)
            .to_http_request();
        let checker = web::Data::new(checker);
        match states(req, checker) {
            Ok(Json(view)) => Ok(view),
            Err(err) => Err(err),
        }
    }
}
