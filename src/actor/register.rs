//! Defines an interface for register-like actors (via `RegisterMsg`) and also provides a wrapper
//! `Actor` (via `RegisterActor`) that implements client behavior for model checking a register
//! implementation.

use crate::actor::system::*;
use crate::actor::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;

/// A wrapper configuration for model-checking a register-like actor.
#[derive(Clone)]
pub enum RegisterActor<Value, ServerActor> {
    Client {
        server_ids: Vec<Id>,
        desired_value: Value,
    },
    Server(ServerActor),
}

/// Defines an interface for a register-like actor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum RegisterMsg<Value, InternalMsg> {
    Put(Value),
    Get,
    Respond(Value),
    Internal(InternalMsg),
}

/// A wrapper state for model-checking a register-like actor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RegisterActorState<ServerState> {
    Client,
    Server(ServerState),
}

impl<Value, Server, ServerMsg> Actor for RegisterActor<Value, Server>
where
    Value: Clone,
    Server: Actor<Msg = RegisterMsg<Value, ServerMsg>>,
    ServerMsg: Serialize + DeserializeOwned,
{
    type Msg = Server::Msg;
    type State = RegisterActorState<Server::State>;

    fn init(i: InitIn<Self>, o: &mut Out<Self>) {
        match i.context {
            RegisterActor::Client {
                ref server_ids,
                ref desired_value,
            } => {
                o.set_state(RegisterActorState::Client);
                for server_id in server_ids {
                    o.send(*server_id, RegisterMsg::Put(desired_value.clone()));
                    o.send(*server_id, RegisterMsg::Get);
                }
            }
            RegisterActor::Server(ref server) => {
                let server_out = server.init_out(i.id);
                o.state = server_out
                    .state
                    .map(|state| RegisterActorState::Server(state));
                o.commands = server_out.commands;
            }
        }
    }

    fn next(i: NextIn<Self>, o: &mut Out<Self>) {
        match (i.context, i.state) {
            (RegisterActor::Server(server), RegisterActorState::Server(server_state)) => {
                let server_out = server.next_out(i.id, server_state, i.event);
                o.state = server_out
                    .state
                    .map(|state| RegisterActorState::Server(state));
                o.commands = server_out.commands;
            }
            _ => {}
        }
    }

    fn deserialize(bytes: &[u8]) -> serde_json::Result<Self::Msg>
    where
        Self::Msg: DeserializeOwned,
    {
        if let Ok(msg) = serde_json::from_slice::<ServerMsg>(bytes) {
            Ok(RegisterMsg::Internal(msg))
        } else {
            serde_json::from_slice(bytes)
        }
    }

    fn serialize(msg: &Self::Msg) -> serde_json::Result<Vec<u8>>
    where
        Self::Msg: Serialize,
    {
        match msg {
            RegisterMsg::Internal(msg) => serde_json::to_vec(msg),
            _ => serde_json::to_vec(msg),
        }
    }
}

/// Indicates unique values with which the server has responded.
pub fn response_values<Value: Clone + Ord, ServerMsg, ServerState>(
    state: &_SystemState<RegisterMsg<Value, ServerMsg>, RegisterActorState<ServerState>>,
) -> Vec<Value> {
    let mut values: Vec<Value> = state
        .network
        .iter()
        .filter_map(|env| match &env.msg {
            RegisterMsg::Respond(value) => Some(value.clone()),
            _ => None,
        })
        .collect();
    values.sort();
    values.dedup();
    values
}
