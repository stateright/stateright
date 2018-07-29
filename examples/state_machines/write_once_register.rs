//! A simple server exposing a single register that can only be written once.

#[allow(unused_imports)] // false warning
use stateright::*;
use stateright::actor::*;
use stateright::actor::register::*;

pub type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ServerState { maybe_value: Option<Value> }

pub struct ServerCfg;

impl<Id> Actor<Id> for ServerCfg {
    type Msg = RegisterMsg<Value, ()>;
    type State = ServerState;

    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
        ActorResult::new(ServerState { maybe_value: None })
    }

    fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
        let ActorInput::Deliver { src, msg } = input;
        match msg {
            RegisterMsg::Put { value } => {
                actor.action = "SERVER ACCEPTS PUT";
                if let None = actor.state.maybe_value {
                    actor.state.maybe_value = Some(value.clone());
                }
            }
            RegisterMsg::Get => {
                actor.action = "SERVER RESPONDS TO GET";
                if let Some(value) = actor.state.maybe_value {
                    actor.outputs.send(src, RegisterMsg::Respond { value: value.clone() });
                }
            }
            _ => {}
        }
    }
}

#[test]
fn can_model_wor() {
    use stateright::actor::model::*;

    let system = ActorSystem {
        actors: vec![
            RegisterCfg::Server(ServerCfg),
            RegisterCfg::Client { server_ids: vec![0], desired_value: 'X' },
            RegisterCfg::Client { server_ids: vec![0], desired_value: 'Y' },
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
