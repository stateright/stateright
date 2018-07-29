//! Defines an interface for register-like actors (via `RegisterMsg`) and also provides a wrapper
//! `Actor` (via `RegisterCfg`) that implements client behavior for model checking a register
//! implementation.

use ::actor::*;
use ::actor::model::*;

/// A wrapper configuration for model-checking a register-like actor.
pub enum RegisterCfg<Id, Value, ServerCfg> {
    Client {
        server_ids: Vec<Id>,
        desired_value: Value,
    },
    Server(ServerCfg),
}

/// Defines an interface for a register-like actor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(Serialize, Deserialize)]
pub enum RegisterMsg<Value, ServerMsg> {
    Put { value: Value },
    Get,
    Respond { value: Value},
    Internal { contents: ServerMsg },
}

/// A wrapper state for model-checking a register-like actor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RegisterState<ServerState> {
    Client,
    Server(ServerState),
}

impl<Id, Value, ServerCfg, ServerMsg> Actor<Id> for RegisterCfg<Id, Value, ServerCfg>
where
    Id: Copy + Ord,
    Value: Clone,
    ServerCfg: Actor<Id, Msg = RegisterMsg<Value, ServerMsg>>,
    ServerCfg::State: Clone,
{
    type Msg = ServerCfg::Msg;
    type State = RegisterState<ServerCfg::State>;

    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
        match self {
            &RegisterCfg::Client { ref server_ids, ref desired_value } => {
                let mut actor = ActorResult::new(RegisterState::Client);
                for server_id in server_ids {
                    actor.outputs.send(*server_id, RegisterMsg::Put { value: desired_value.clone() });
                    actor.outputs.send(*server_id, RegisterMsg::Get);
                }
                actor
            }
            &RegisterCfg::Server(ref server_cfg) => {
                let result = server_cfg.start();
                let mut actor = ActorResult::new(RegisterState::Server(result.state));
                for output in result.outputs.0 {
                    let ActorOutput::Send { dst, msg } = output;
                    actor.outputs.send(dst, msg);
                }
                actor
            }
        }
    }

    fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
        if let RegisterCfg::Server(server_cfg) = self {
            if let RegisterState::Server(ref mut server_state) = &mut actor.state {
                // `ActorResult` takes ownership of state, so we're forced to clone.
                let mut result = ActorResult::new(server_state.clone());
                server_cfg.advance(input, &mut result);
                for output in result.outputs.0 {
                    let ActorOutput::Send { dst, msg } = output;
                    actor.outputs.send(dst, msg);
                }
                *server_state = result.state.clone();
            }
        }
    }
}

/// Indicates unique values with which the server has responded.
pub fn response_values<Value: Clone + Ord, ServerMsg, ServerState>(
    state: &ActorSystemSnapshot<
        RegisterMsg<Value, ServerMsg>,
        RegisterState<ServerState>
    >) -> Vec<Value> {
    let mut values: Vec<Value> = state.network.iter().filter_map(
        |env| match &env.msg {
            RegisterMsg::Respond { value } => Some(value.clone()),
            _ => None,
        }).collect();
    values.sort();
    values.dedup();
    values
}

