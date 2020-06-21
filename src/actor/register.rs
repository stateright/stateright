//! Defines an interface for register-like actors (via `RegisterMsg`) and also provides a wrapper
//! `Actor` (via `RegisterActor`) that implements client behavior for model checking a register
//! implementation.

use crate::actor::*;
use crate::actor::system::*;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::hash::Hash;

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
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
pub enum RegisterMsg<Value, InternalMsg> {
    Put(Value),
    Get,
    Respond(Value),
    Internal(InternalMsg),
}

/// A wrapper state for model-checking a register-like actor.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RegisterActorState<ServerState> {
    Client,
    Server(ServerState),
}

impl<Value, Server, ServerMsg> Actor for RegisterActor<Value, Server>
where
    Value: Clone + Debug + Eq + Hash,
    Server: Actor<Msg = RegisterMsg<Value, ServerMsg>>,
    ServerMsg: Clone + Debug + Eq + Hash + Serialize + DeserializeOwned,
{
    type Msg = Server::Msg;
    type State = RegisterActorState<Server::State>;

    fn on_start(&self, id: Id, o: &mut Out<Self>) {
        match self {
            RegisterActor::Client { ref server_ids, ref desired_value } => {
                o.set_state(RegisterActorState::Client);
                for server_id in server_ids {
                    o.send(*server_id, RegisterMsg::Put(desired_value.clone()));
                    o.send(*server_id, RegisterMsg::Get);
                }
            }
            RegisterActor::Server(ref server) => {
                let server_out = server.on_start_out(id);
                o.state = server_out.state.map(|state| RegisterActorState::Server(state));
                o.commands = server_out.commands;
            }
        }
    }

    fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        if let (RegisterActor::Server(server), RegisterActorState::Server(server_state)) = (self, state) {
            let server_out = server.on_msg_out(id, server_state, src, msg);
            o.state = server_out.state.map(|state| RegisterActorState::Server(state));
            o.commands = server_out.commands;
        }
    }

    fn on_timeout(&self, id: Id, state: &Self::State, o: &mut Out<Self>) {
        if let (RegisterActor::Server(server), RegisterActorState::Server(server_state)) = (self, state) {
            let server_out = server.on_timeout_out(id, server_state);
            o.state = server_out.state.map(|state| RegisterActorState::Server(state));
            o.commands = server_out.commands;
        }
    }

    fn deserialize(bytes: &[u8]) -> serde_json::Result<Self::Msg> where Self::Msg: DeserializeOwned {
        if let Ok(msg) = serde_json::from_slice::<ServerMsg>(bytes) {
            Ok(RegisterMsg::Internal(msg))
        } else {
            serde_json::from_slice(bytes)
        }
    }

    fn serialize(msg: &Self::Msg) -> serde_json::Result<Vec<u8>> where Self::Msg: Serialize {
        match msg {
            RegisterMsg::Internal(msg) => serde_json::to_vec(msg),
            _ => serde_json::to_vec(msg),
        }
    }
}

/// Indicates unique values with which the server has responded.
pub fn response_values<Value: Clone + Hash + Ord, ServerMsg: Eq + Hash, ServerState>(
    state: &_SystemState<
        RegisterMsg<Value, ServerMsg>,
        RegisterActorState<ServerState>
    >) -> Vec<Value> {
    let mut values: Vec<Value> = state.network.iter().filter_map(
        |env| match &env.msg {
            RegisterMsg::Respond(value) => Some(value.clone()),
            _ => None,
        }).collect();
    values.sort();
    values.dedup();
    values
}
