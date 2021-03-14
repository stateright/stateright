use actix_web::{*, web::Json};
use crate::*;
use parking_lot::RwLock;
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::thread::{sleep, spawn};
use std::time::Duration;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct StatusView {
    done: bool,
    model: String,
    generated: usize,
    discoveries: BTreeMap<String, String>, // name+classification -> encoded path
    recent_path: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct StateView<State, Action> {
    action: Option<Action>,
    outcome: Option<String>,
    state: State,
    svg: Option<String>,
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
        if let Some(ref svg) = self.svg {
            out.serialize_field("svg", svg)?;
        }
        out.serialize_field("fingerprint", &format!("{:?}", fingerprint(&self.state)))?;
        out.end()
    }
}

struct Snapshot<Action>(bool, Option<Vec<Action>>);
impl<M: Model> CheckerVisitor<M> for Arc<RwLock<Snapshot<M::Action>>> {
    fn visit(&self, _: &M, path: Path<M::State, M::Action>) {
        let guard = self.read();
        if !guard.0 { return }
        drop(guard);

        let mut guard = self.write();
        if !guard.0 { return } // May be racing other threads.
        guard.0 = false;
        guard.1 = Some(path.into_actions());
    }
}

pub(crate) fn serve<M>(checker_builder: CheckerBuilder<M>, addresses: impl ToSocketAddrs) -> Arc<impl Checker<M>>
where M: 'static + Model + Send + Sync,
      M::Action: Debug + Send + Sync,
      M::State: Debug + Hash + Send + Sync,
{
    let snapshot = Arc::new(RwLock::new(Snapshot(true, None)));
    let snapshot_for_visitor = Arc::clone(&snapshot);
    let snapshot_for_server = Arc::clone(&snapshot);
    spawn(move || {
        loop {
            sleep(Duration::from_secs(4));
            snapshot.write().0 = true;
        }
    });
    let checker = checker_builder
        .visitor(snapshot_for_visitor)
        .spawn_bfs();
    serve_checker(checker, snapshot_for_server, addresses)
}

fn serve_checker<M, C>(
    checker: C,
    snapshot: Arc<RwLock<Snapshot<M::Action>>>,
    addresses: impl ToSocketAddrs)
    -> Arc<impl Checker<M>>
where M: 'static + Model + Send + Sync,
      M::Action: Debug + Send + Sync,
      M::State: Debug + Hash + Send + Sync,
      C: 'static + Checker<M> + Send + Sync,
{
    let checker = Arc::new(checker);

    let data = Arc::new((snapshot, Arc::clone(&checker)));
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

type Data<Action, Checker> = web::Data<Arc<(Arc<RwLock<Snapshot<Action>>>, Arc<Checker>)>>;

fn status<M, C>(_: HttpRequest, data: Data<M::Action, C>) -> Result<Json<StatusView>>
where M: Model,
      M::Action: Debug,
      M::State: Hash,
      C: Checker<M>,
{
    let snapshot = &data.0;
    let checker = &data.1;

    let status = StatusView {
        model: std::any::type_name::<M>().to_string(),
        done: checker.is_done(),
        generated: checker.generated_count(),
        discoveries: checker.discoveries().into_iter()
            .map(|(name, path)| {
                let key = format!("\"{}\" {}", name, checker.discovery_classification(name));
                let value = path.encode();
                (key, value)
            })
            .collect(),
        recent_path: snapshot.read().1.as_ref().map(|p| format!("{:?}", p)),
    };
    Ok(Json(status))
}

fn states<M, C>(req: HttpRequest, data: Data<M::Action, C>)
    -> Result<StateViewsJson<M::State, M::Action>>
where M: Model,
      M::State: Debug + Hash,
      C: Checker<M>,
{
    let model = &data.1.model();

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
            let svg = {
                let mut fingerprints: VecDeque<_> = fingerprints.clone().into_iter().collect();
                fingerprints.push_back(fingerprint(&state));
                model.as_svg(Path::from_fingerprints::<M>(model, fingerprints))
            };
            results.push(StateView {
                action: None,
                outcome: None,
                state,
                svg,
            });
        }
    } else if let Some(last_state) = Path::final_state::<M>(model, fingerprints.clone()) {
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
                let svg = {
                    let mut fingerprints: VecDeque<_> = fingerprints.clone().into_iter().collect();
                    fingerprints.push_back(fingerprint(&state));
                    model.as_svg(Path::from_fingerprints::<M>(model, fingerprints))
                };
                results.push(StateView { action: Some(action), outcome, state, svg });
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
            StateView { action: None, outcome: None, state: 0, svg: None },
            StateView { action: None, outcome: None, state: 1, svg: None },
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
            StateView {
                action: Some(BinaryClockAction::GoHigh),
                outcome: Some("1".to_string()),
                state: 1,
                svg: None,
            },
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
    fn smoke_test_states() {
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
                    svg: Some("<svg version=\'1.1\' baseProfile=\'full\' width=\'500\' height=\'30\' viewbox=\'-20 -20 520 50\' xmlns=\'http://www.w3.org/2000/svg\'><defs><marker class=\'svg-event-shape\' id=\'arrow\' markerWidth=\'12\' markerHeight=\'10\' refX=\'12\' refY=\'5\' orient=\'auto\'><polygon points=\'0 0, 12 5, 0 10\' /></marker></defs><line x1=\'0\' y1=\'0\' x2=\'0\' y2=\'30\' class=\'svg-actor-timeline\' />\n<text x=\'0\' y=\'0\' class=\'svg-actor-label\'>0</text>\n<line x1=\'100\' y1=\'0\' x2=\'100\' y2=\'30\' class=\'svg-actor-timeline\' />\n<text x=\'100\' y=\'0\' class=\'svg-actor-label\'>1</text>\n</svg>\n".to_string()),
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
        let states = get_states(Arc::clone(&checker), "/2298670378538534683").unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(
            states[0],
            StateView {
                action: Some(Drop(Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) })),
                outcome: Some("DROP: Envelope { src: Id(0), dst: Id(1), msg: Ping(0) }".to_string()),
                state: SystemState {
                    actor_states: vec![Arc::new(PingPongCount(0)), Arc::new(PingPongCount(0))],
                    history: (0, 1),
                    is_timer_set: vec![],
                    network: HashableHashSet::new(),
                },
                svg: Some("<svg version='1.1' baseProfile='full' width='500' height='60' viewbox='-20 -20 520 80' xmlns='http://www.w3.org/2000/svg'><defs><marker class='svg-event-shape' id='arrow' markerWidth='12' markerHeight='10' refX='12' refY='5' orient='auto'><polygon points='0 0, 12 5, 0 10' /></marker></defs><line x1='0' y1='0' x2='0' y2='60' class='svg-actor-timeline' />\n<text x='0' y='0' class='svg-actor-label'>0</text>\n<line x1='100' y1='0' x2='100' y2='60' class='svg-actor-timeline' />\n<text x='100' y='0' class='svg-actor-label'>1</text>\n</svg>\n".to_string()),
            });
        assert_eq!(
            states[1],
            StateView {
                action: Some(Deliver { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }),
                outcome: Some("OUT: [Send(Id(0), Pong(0))]\n\nNEXT_STATE: PingPongCount(\n    1,\n)\n\nPREV_STATE: PingPongCount(\n    0,\n)\n".to_string()),
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
                },
                svg: Some("<svg version='1.1' baseProfile='full' width='500' height='60' viewbox='-20 -20 520 80' xmlns='http://www.w3.org/2000/svg'><defs><marker class='svg-event-shape' id='arrow' markerWidth='12' markerHeight='10' refX='12' refY='5' orient='auto'><polygon points='0 0, 12 5, 0 10' /></marker></defs><line x1='0' y1='0' x2='0' y2='60' class='svg-actor-timeline' />\n<text x='0' y='0' class='svg-actor-label'>0</text>\n<line x1='100' y1='0' x2='100' y2='60' class='svg-actor-timeline' />\n<text x='100' y='0' class='svg-actor-label'>1</text>\n<line x1='0' x2='100' y1='0' y2='30' marker-end='url(#arrow)' class='svg-event-line' />\n<text x='100' y='30' class='svg-event-label'>Ping(0)</text>\n</svg>\n".to_string()),
            });
    }

    #[test]
    fn smoke_test_status() {
        use crate::actor::{DuplicatingNetwork, LossyNetwork, System};
        use crate::actor::actor_test_util::ping_pong::PingPongSystem;

        let snapshot = Arc::new(RwLock::new(Snapshot(true, None)));
        let checker = PingPongSystem {
            max_nat: 2,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::No,
            maintains_history: true,
        }.into_model().checker()
            .visitor(Arc::clone(&snapshot)).spawn_bfs().join();
        let status = get_status(Arc::new(checker), snapshot).unwrap();
        assert_eq!(status.done, true);
        assert_eq!(
            status.model,
           "stateright::actor::system::SystemModel<\
                stateright::actor::actor_test_util::ping_pong::PingPongSystem\
            >");
        assert_eq!(status.generated, 5);
        assert_eq!(status.discoveries.len(), 2);
        assert!(status.discoveries.get("\"can reach max\" example").is_some());
        assert!(status.discoveries.get("\"must exceed max\" counterexample").is_some());
        assert!(status.recent_path.unwrap().starts_with("["));
    }

    fn get_states<M, C>(checker: Arc<C>, path_name: &'static str)
                -> Result<Vec<StateView<M::State, M::Action>>>
    where M: Model,
          M::State: Debug + Hash,
          C: Checker<M>,
    {
        let req = actix_web::test::TestRequest::get()
            .param("fingerprints", &path_name)
            .to_http_request();
        let snapshot = Arc::new(RwLock::new(Snapshot(true, None)));
        let data = web::Data::new(Arc::new((snapshot, checker)));
        match states(req, data) {
            Ok(Json(view)) => Ok(view),
            Err(err) => Err(err),
        }
    }

    fn get_status<M, C>(checker: Arc<C>, snapshot: Arc<RwLock<Snapshot<M::Action>>>)
    -> Result<StatusView>
    where M: Model,
          M::Action: Debug,
          M::State: Debug + Hash,
          C: Checker<M>,
    {
        let req = actix_web::test::TestRequest::get().to_http_request();
        let data = web::Data::new(Arc::new((snapshot, checker)));
        match status(req, data) {
            Ok(Json(view)) => Ok(view),
            Err(err) => Err(err),
        }
    }
}
