//! Defines an interface for register-like actors (via [`RegisterMsg`]) and also provides
//! [`RegisterTestSystem`] for model checking.

use crate::Property;
use crate::actor::{Actor, Id, Out};
use crate::actor::system::{DuplicatingNetwork, LossyNetwork, System, SystemModel, SystemState};
use crate::semantics::register::{Register, RegisterOp, RegisterRet};
use crate::semantics::LinearizabilityTester;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::hash::Hash;

/// Defines an interface for a register-like actor.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
pub enum RegisterMsg<RequestId, Value, InternalMsg> {
    /// A message specific to the register system's internal protocol.
    Internal(InternalMsg),

    /// Indicates that a value should be written.
    Put(RequestId, Value),
    /// Indicates that a value should be retrieved.
    Get(RequestId),

    /// Indicates a successful `Put`. Analogous to an HTTP 2XX.
    PutOk(RequestId),
    /// Indicates a successful `Get`. Analogous to an HTTP 2XX.
    GetOk(RequestId, Value),
}
use RegisterMsg::*;

/// A system for testing an actor service with register semantics.
#[derive(Clone)]
pub struct RegisterTestSystem<ServerActor, InternalMsg>
where
    ServerActor: Actor<Msg = RegisterMsg<TestRequestId, TestValue, InternalMsg>> + Clone,
    InternalMsg: Clone + Debug + Eq + Hash,
{
    pub servers: Vec<ServerActor>,
    pub client_count: u8,
    pub within_boundary: fn(state: &SystemState<Self>) -> bool,
    pub lossy_network: LossyNetwork,
    pub duplicating_network: DuplicatingNetwork,
}

impl<ServerActor, InternalMsg> Default for RegisterTestSystem<ServerActor, InternalMsg>
    where
    ServerActor: Actor<Msg = RegisterMsg<TestRequestId, TestValue, InternalMsg>> + Clone,
    InternalMsg: Clone + Debug + Eq + Hash,
{
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            client_count: 2,
            within_boundary: |_| true,
            lossy_network: LossyNetwork::No,
            duplicating_network: DuplicatingNetwork::Yes,
        }
    }
}

impl<ServerActor, InternalMsg> System for RegisterTestSystem<ServerActor, InternalMsg>
    where
        ServerActor: Actor<Msg = RegisterMsg<TestRequestId, TestValue, InternalMsg>> + Clone,
        InternalMsg: Clone + Debug + Eq + Hash,
{
    type Actor = RegisterActor<ServerActor>;
    type History = LinearizabilityTester<Id, Register<TestValue>>;

    fn actors(&self) -> Vec<Self::Actor> {
        let mut actors: Vec<Self::Actor> = self.servers.iter().map(|s| {
            RegisterActor::Server(s.clone())
        }).collect();
        for _ in 0..self.client_count {
            actors.push(RegisterActor::Client { server_count: self.servers.len() as u64 });
        }
        actors
    }

    fn lossy_network(&self) -> LossyNetwork {
        self.lossy_network
    }

    fn duplicating_network(&self) -> DuplicatingNetwork {
        self.duplicating_network
    }

    fn record_msg_in(&self, history: &Self::History, src: Id, _dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        // FIXME: Currently panics for invalid histories. Ideally checking
        //        would continue, but the property would be labeled with an
        //        error.
        if let Get(_) = msg {
            let mut history = history.clone();
            history.on_invoke(src, RegisterOp::Read).unwrap();
            Some(history)
        } else if let Put(_req_id, value) = msg {
            let mut history = history.clone();
            history.on_invoke(src, RegisterOp::Write(*value)).unwrap();
            Some(history)
        } else {
            None
        }
    }

    fn record_msg_out(&self, history: &Self::History, _src: Id, dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        // FIXME: Currently panics for invalid histories. Ideally checking
        //        would continue, but the property would be labeled with an
        //        error.
        match msg {
            GetOk(_, v) => {
                let mut history = history.clone();
                history.on_return(dst, RegisterRet::ReadOk(v.clone())).unwrap();
                Some(history)
            }
            PutOk(_) => {
                let mut history = history.clone();
                history.on_return(dst, RegisterRet::WriteOk).unwrap();
                Some(history)
            }
            _ => None
        }
    }

    fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
        vec![
            Property::<SystemModel<Self>>::always("linearizable", |_, state| {
                state.history.serialized_history().is_some()
            }),
            Property::<SystemModel<Self>>::sometimes("value chosen",  |_, state| {
                for env in &state.network {
                    if let RegisterMsg::GetOk(_req_id, value) = env.msg {
                        if value != TestValue::default() { return true; }
                    }
                }
                false
            }),
        ]
    }

    fn within_boundary(&self, state: &SystemState<Self>) -> bool {
        (self.within_boundary)(state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterActor<ServerActor> {
    /// A client that [`RegisterMsg::Put`]s a message and upon receving a
    /// corresponding [`RegisterMsg::PutOk`] follows up with a
    /// [`RegisterMsg::Get`].
    Client {
        server_count: u64,
    },
    /// A server actor being validated.
    Server(ServerActor),
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RegisterActorState<ServerState> {
    /// Indicates that the client sent a [`RegisterMsg::Put`] and will later
    /// send a [`RegisterMsg::Get`].
    ClientIncomplete,
    /// Indicates that the client has no more messages to send.
    ClientComplete,
    /// Wraps the state of a server actor.
    Server(ServerState),
}

// This implementation assumes the servers are at the beginning of the list of
// actors in the system under test so that an arbitrary server destination ID
// can be derived from `(client_id.0 + k) % server_count` for any `k`.
impl<ServerActor, InternalMsg> Actor for RegisterActor<ServerActor>
where
    ServerActor: Actor<Msg = RegisterMsg<TestRequestId, TestValue, InternalMsg>>,
    InternalMsg: Clone + Debug + Eq + Hash,
{
    type Msg = RegisterMsg<TestRequestId, TestValue, InternalMsg>;
    type State = RegisterActorState<ServerActor::State>;

    fn on_start(&self, id: Id, o: &mut Out<Self>) {
        match self {
            RegisterActor::Client { server_count } => {
                let index = id.0;
                let unique_request_id = 1 * index as TestRequestId; // next will be 2 * index
                o.state = Some(RegisterActorState::ClientIncomplete);
                o.send(
                    Id((index + 0) % server_count),
                    Put(unique_request_id, (b'A' + (index - server_count) as u8) as char));
            }
            RegisterActor::Server(server_actor) => {
                let server_out = server_actor.on_start_out(id);
                o.state = server_out.state.map(RegisterActorState::Server);
                o.commands = server_out.commands;
            }
        }
    }

    fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match (self, state) {
            (RegisterActor::Client { server_count }, RegisterActorState::ClientIncomplete) => {
                let index = id.0;
                let unique_request_id = 2 * index as TestRequestId;
                o.state = Some(RegisterActorState::ClientComplete);
                o.send(
                    Id((index + 1) % server_count),
                    Get(unique_request_id));
            }
            (RegisterActor::Server(server_actor), RegisterActorState::Server(server_state)) => {
                let server_out = server_actor.on_msg_out(id, server_state, src, msg);
                o.state = server_out.state.map(RegisterActorState::Server);
                o.commands = server_out.commands;
            }
            _ => {}
        }
    }
}


/// A simple request ID type for tests.
pub type TestRequestId = u64;

/// A simple value type for tests.
pub type TestValue = char;
