//! A simple server exposing a single register that can only be written once.

use stateright::*;
use stateright::actor::*;

pub type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Msg {
    Put { value: Value },
    Get,
    Respond { value: Value },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum State {
    Client,
    Server { maybe_value: Option<Value> },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Actor<Id> {
    Client { desired_value: Value, server_id: Id },
    Server,
}

impl<Id: Copy> actor::Actor<Id> for Actor<Id> {
    type Msg = Msg;
    type State = State;

    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
        match self {
            Actor::Client { desired_value, server_id } => {
                let mut result = ActorResult::new(State::Client);
                result.outputs.send(*server_id, Msg::Put { value: desired_value.clone() });
                result.outputs.send(*server_id, Msg::Get);
                result
            },
            Actor::Server => ActorResult::new(State::Server { maybe_value: None }),
        }
    }

    fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
        let ActorInput::Deliver { src, msg } = input;
        if let State::Server { ref mut maybe_value } = actor.state {
            match msg {
                Msg::Put { value } => {
                    if let None = maybe_value {
                        *maybe_value = Some(value.clone());
                    }
                }
                Msg::Get => {
                    if let Some(value) = maybe_value {
                        actor.outputs.send(src, Msg::Respond { value: value.clone() });
                    }
                }
                _ => {}
            }
        }
    }
}

#[test]
fn can_model_wor() {
    use stateright::actor::model::*;
    let system = ActorSystem {
        actors: vec![
            Actor::Server,
            Actor::Client { server_id: 0, desired_value: 'X' },
            Actor::Client { server_id: 0, desired_value: 'Y' },
        ],
        init_network: Vec::new(),
    };
    let mut checker = system.checker(KeepPaths::Yes, |_sys, state| {
        // only returns a value in the set of values proposed by clients
        state.network.iter().all(
            |env| match env.msg {
                Msg::Respond { value } => value == 'X' || value == 'Y',
                _ => true,
            })
    });
    assert_eq!(checker.check(10_000), CheckResult::Pass);
    assert_eq!(checker.visited.len(), 144);
}

