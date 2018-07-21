//! A simple server exposing a single register that can only be written once.

#[allow(unused_imports)] // false warning
use stateright::*;
use stateright::actor::*;
use stateright::actor::model::*;

pub type Value = char;

actor! {
    Cfg<Id> {
        #[allow(dead_code)] // not constructed here (only used for model checking)
        Client { desired_value: Value, server_id: Id },
        Server,
    }
    State {
        Client,
        Server { maybe_value: Option<Value> },
    }
    Msg {
        Put { value: Value },
        Get,
        Respond { value: Value },
    }
    Start() {
        Cfg::Client { desired_value, server_id } => {
            let mut result = ActorResult::new(State::Client);
            result.outputs.send(*server_id, Msg::Put { value: desired_value.clone() });
            result.outputs.send(*server_id, Msg::Get);
            result
        },
        Cfg::Server => ActorResult::new(State::Server { maybe_value: None }),
    }
    Advance(src, msg, actor) {
        Cfg::Server => {
            if let State::Server { ref mut maybe_value } = actor.state {
                match msg {
                    Msg::Put { value } => {
                        actor.action = "SERVER ACCEPTS PUT";
                        if let None = maybe_value {
                            *maybe_value = Some(value.clone());
                        }
                    }
                    Msg::Get => {
                        actor.action = "SERVER RESPONDS TO GET";
                        if let Some(value) = maybe_value {
                            actor.outputs.send(src, Msg::Respond { value: value.clone() });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Indicates unique values with which the server has responded.
#[allow(dead_code)] // not used by `serve.rs`
pub fn response_values(state: &ActorSystemSnapshot<Msg, State>) -> Vec<Value> {
    let mut values: Vec<Value> = state.network.iter().filter_map(
        |env| match env.msg {
            Msg::Respond { value } => Some(value),
            _ => None,
        }).collect();
    values.sort();
    values.dedup();
    values
}

#[test]
fn can_model_wor() {
    let system = ActorSystem {
        actors: vec![
            Cfg::Server,
            Cfg::Client { server_id: 0, desired_value: 'X' },
            Cfg::Client { server_id: 0, desired_value: 'Y' },
        ],
        init_network: Vec::new(),
    };
    let mut checker = system.checker(KeepPaths::Yes, |_sys, state| {
        let values = response_values(&state);
        match values.as_slice() {
            [] => true,
            [v] => *v == 'X' || *v == 'Y',
            _ => false
        }
    });
    assert_eq!(checker.check(10_000), CheckResult::Pass);
    assert_eq!(checker.visited.len(), 144);
}

