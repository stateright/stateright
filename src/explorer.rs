//! A web service for interactively exploring a model.
//!
//! ![Stateright Explorer screenshot](https://raw.githubusercontent.com/stateright/stateright/master/explorer.jpg)
//!
//! API summary:
//! - `GET /` returns a web browser UI as HTML.
//! - `GET /.status` returns information about the model checker status.
//! - `GET /.states` returns available initial states and fingerprints.
//! - `GET /.states/{fingerprint1}/{fingerprint2}/...` follows the specified
//!    path of fingerprints and returns available actions with resulting
//!    states and fingerprints.
//! - `GET /.states/.../{invalid-fingerprint}` returns 404.

use actix_web::{*, web::Json};
use crate::*;
use crate::checker::PathName;
use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

#[derive(Debug, Eq, PartialEq)]
pub struct StateView<State, Action> {
    action: Option<Action>,
    outcome: Option<String>,
    state: State,
}

impl<Action, State> StateView<State, Action>
where State: Hash
{
    fn fingerprint(&self) -> Fingerprint {
        fingerprint(&self.state)
    }
}

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
        out.serialize_field("fingerprint", &format!("{:?}", self.fingerprint()))?;
        out.end()
    }
}

pub type StateViewsJson<State, Action> = Json<Vec<StateView<State, Action>>>;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StatusView {
    model: String,
    threads: usize,
    pending: usize,
    generated: usize,
    discoveries: HashMap<&'static str, PathName>,
}

pub struct Context<M> {
    model: M,
    status: Mutex<StatusView>,
}

impl<M: Model> Explorer<M> for Checker<M>
where M: 'static + Clone + Model + Send + Sync,
      M::Action: Debug + Send + Sync,
      M::State: Clone + Debug + Hash + Send + Sync,
{

    /// Begin serving requests on a specified address such as `"localhost:3000"`.
    fn serve<A: ToSocketAddrs>(mut self, addr: A) -> Result<()>
    {
        self.check(1_000); // small number just to expedite startup
        let data = Arc::new(Context {
            model: self.model.clone(),
            status: Mutex::new(self.status_view()),
        });

        {
            let data = Arc::clone(&data);
            std::thread::spawn(move || {
                let context = &*data;
                loop {
                    self.check(25_000);
                    let mut status = context.status.lock().unwrap();
                    *status = self.status_view();
                    if self.is_done() { break }
                }
            });
        }

        HttpServer::new(move || {
            macro_rules! get_ui_file {
                ($filename:literal) => {
                    web::get().to(|| HttpResponse::Ok().body({
                        if let Ok(content) = std::fs::read(concat!("./ui/", $filename)) {
                            log::info!("Explorer dev mode. Loading {} from disk.", $filename);
                            content
                        } else {
                            include_bytes!(concat!("../ui/", $filename)).to_vec()
                        }
                    }))
                }
            };

            App::new()
                .data(Arc::clone(&data))
                .route("/.status", web::get().to(Self::status))
                .route("/.states{fingerprints:.*}", web::get().to(Self::states))
                .route("/", get_ui_file!("index.htm"))
                .route("/app.css", get_ui_file!("app.css"))
                .route("/app.js", get_ui_file!("app.js"))
                .route("/knockout-3.5.0.js", get_ui_file!("knockout-3.5.0.js"))
        }).bind(addr)?.run()?;

        Ok(())
    }

    fn status_view(&self) -> StatusView {
        StatusView {
            model: std::any::type_name::<M>().to_string(),
            threads: self.thread_count,
            pending: self.pending.len(),
            generated: self.generated_count(),
            discoveries: self.discoveries.iter()
                .map(|e| (
                    *e.key(),
                    self.path(*e.value()).name(),
                )).collect(),
        }
    }
}

pub trait Explorer<M>: Sized + Send + 'static
where
    M: 'static + Model + Send + Sync,
    M::Action: Debug + Send + Sync,
    M::State: Clone + Debug + Hash + Send + Sync,
{
    /// Begin serving requests on a specified address such as `"localhost:3000"`.
    fn serve<A: ToSocketAddrs>(self, addr: A) -> Result<()>;

    fn status(_req: HttpRequest, data: web::Data<Arc<Context<M>>>) -> Result<Json<StatusView>> {
        let status = data.status.lock().unwrap();
        Ok(Json(status.clone()))
    }

    fn states(req: HttpRequest, data: web::Data<Arc<Context<M>>>) -> Result<StateViewsJson<M::State, M::Action>> {
        let model = &data.model;

        // extract fingerprints
        let mut fingerprints_str = req.match_info().get("fingerprints").expect("missing 'fingerprints' param").to_string();
        if fingerprints_str.ends_with('/') {
            let relevant_len = fingerprints_str.len() - 1;
            fingerprints_str.truncate(relevant_len);
        }
        let fingerprints: Vec<_> = fingerprints_str.split('/').filter_map(|fp| fp.parse::<Fingerprint>().ok()).collect();

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
        } else if let Some(last_state) = model.follow_fingerprints(model.init_states(), fingerprints) {
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
                if let (Some(outcome), Some(state)) = (outcome, state) {
                    results.push(StateView { action: Some(action), outcome: Some(outcome), state });
                }
            }
        } else {
            return Err(
                actix_web::error::ErrorNotFound(
                    format!("Unable to find state following fingerprints {}", fingerprints_str)));
        }

        Ok(Json(results))
    }

    fn status_view(&self) -> StatusView;
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::binary_clock::*;

    #[test]
    fn can_init() {
        assert_eq!(states("/").unwrap(), vec![
            StateView { action: None, outcome: None, state: 0 },
            StateView { action: None, outcome: None, state: 1 },
        ]);
    }

    #[test]
    fn can_next() {
        // We need a static string for TestRequest, so this is precomputed, but you can recompute
        // the values if needed as follows:
        // ```
        // let first = fingerprint(&1_i8);
        // let second = fingerprint(&0_i8);
        // let path_name = format!("/{}/{}", first, second);
        // println!("New path name is: {}", path_name);
        // ```
        assert_eq!(states("/2716592049047647680/9080728272894440685").unwrap(), vec![
            StateView { action: Some(BinaryClockAction::GoHigh), outcome: Some("1".to_string()), state: 1 },
        ]);
    }

    #[test]
    fn err_for_invalid_fingerprint() {
        assert_eq!(format!("{}", states("/one/two/three").unwrap_err()),
            "Unable to parse fingerprints /one/two/three");
        assert_eq!(format!("{}", states("/1/2/3").unwrap_err()),
            "Unable to find state following fingerprints /1/2/3");
    }

    fn states(path_name: &'static str)
            -> Result<Vec<StateView<BinaryClockState, BinaryClockAction>>> {
        use actix_web::test::*;
        let req = TestRequest::get()
            .param("fingerprints", &path_name)
            .to_http_request();
        let data = web::Data::new(Arc::new(Context {
            model: BinaryClock,
            status: Mutex::new(Default::default())
        }));
        match Checker::states(req, data) {
            Ok(Json(view)) => Ok(view),
            Err(err) => Err(err),
        }
    }
}
