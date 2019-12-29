//! A web service for interactively exploring a state machine.
//!
//! API summary:
//! - `GET /.states` returns available initial states and fingerprints.
//! - `GET /.states/{fingerprint1}/{fingerprint2}/...` follows the specified
//!   path of fingerprints and returns available actions with resulting
//!   states and fingerprints.
//! - `GET /.states/.../{invalid-fingerprint}` returns 404.

use actix_web::{*, web::Json};
use crate::*;
use serde::ser::{SerializeStruct, Serializer};
use std::net::ToSocketAddrs;

/// Provides a web service for interactively exploring a state machine.
pub struct Explorer<SM>(pub SM);

/// Summarizes a state and the action that was taken to obtain that state.
#[derive(Debug, Eq, PartialEq)]
pub struct StateView<State, Action> {
    state: State,
    action: Option<Action>,
}

impl<Action, State> StateView<State, Action>
where State: Hash
{
    fn fingerprint(&self) -> Fingerprint {
        hash(&self.state)
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
        out.serialize_field("state", &format!("{:?}", self.state))?;
        out.serialize_field("fingerprint", &format!("{:?}", self.fingerprint()))?;
        out.end()
    }
}

impl<SM> Explorer<SM>
where
    SM: 'static + Send + StateMachine,
    SM::Action: Debug,
    SM::State: Clone + Debug + Hash,
{
    /// Begin serving requests on a specified address such as `"localhost:3000"`.
    pub fn serve<A: ToSocketAddrs>(self, addr: A) -> Result<()>
    where SM: Clone
    {
        let Explorer(sm) = self;
        HttpServer::new(move || {
            App::new()
                .data(sm.clone())
                .route("/.states{fingerprints:.*}", web::get().to(Self::states))
                .service(actix_files::Files::new("/", "./ui").index_file("index.htm"))
        })
        .bind(addr)?
        .run()?;

        Ok(())
    }

    fn states(req: HttpRequest, sys: web::Data<SM>) -> Result<Json<Vec<StateView<SM::State, SM::Action>>>> {
        // extract fingerprints
        let mut fingerprints_str = req.match_info().get("fingerprints").expect("missing 'fingerprints' param").to_string();
        if fingerprints_str.ends_with("/") {
            let relevant_len = fingerprints_str.len() - 1;
            fingerprints_str.truncate(relevant_len);
        }
        let fingerprints: Vec<_> = fingerprints_str.split("/").filter_map(|fp| fp.parse::<Fingerprint>().ok()).collect();

        // ensure all but the first string (which is empty) were parsed
        if fingerprints.len() + 1 != fingerprints_str.split("/").count() {
            return Err(
                actix_web::error::ErrorNotFound(
                    format!("Unable to parse fingerprints {}", fingerprints_str)));
        }

        let mut results = Vec::new();

        if fingerprints.is_empty() {
            for init_state in sys.init_states() {
                results.push(StateView {
                    action: None,
                    state: init_state,
                });
            }
        } else if let Some(last_state) = sys.follow_fingerprints(sys.init_states(), fingerprints) {
            let steps = sys.next_steps(&last_state);
            for (action, next_state) in steps {
                results.push(StateView {
                    action: Some(action),
                    state: next_state,
                });
            }
        } else {
            return Err(
                actix_web::error::ErrorNotFound(
                    format!("Unable to find state following fingerprints {}", fingerprints_str)));
        }

        return Ok(Json(results));
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::binary_clock::*;

    #[test]
    fn can_init() {
        assert_eq!(states("/").unwrap(), vec![
            StateView { action: None, state: 0 },
            StateView { action: None, state: 1 },
        ]);
    }

    #[test]
    fn can_next() {
        assert_eq!(states("/5871781006564002453/0").unwrap(), vec![
            StateView { action: Some(BinaryClockAction::GoHigh), state: 1 },
        ]);
    }

    #[test]
    fn err_for_invalid_fingerprint() {
        assert_eq!(format!("{}", states("/one/two/three").unwrap_err()),
            "Unable to parse fingerprints /one/two/three");
        assert_eq!(format!("{}", states("/1/2/3").unwrap_err()),
            "Unable to find state following fingerprints /1/2/3");
    }

    fn states(fingerprints: &'static str)
            -> Result<Vec<StateView<BinaryClockState, BinaryClockAction>>> {
        use actix_web::test::*;
        let req = TestRequest::get()
            .param("fingerprints", &fingerprints)
            .to_http_request();
        let data = web::Data::new(BinaryClock);
        match Explorer::states(req, data) {
            Ok(Json(view)) => Ok(view),
            Err(err) => Err(err),
        }
    }
}
