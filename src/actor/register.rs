//! Defines an interface for register-like actors (via [`RegisterMsg`]) and also provides
//! [`RegisterActor`] for model checking.

#[cfg(doc)]
use crate::actor::ActorModel;
use crate::actor::{Actor, Envelope, Id, Out};
use crate::semantics::register::{Register, RegisterOp, RegisterRet};
use crate::semantics::ConsistencyTester;
use std::borrow::Cow;
use std::fmt::Debug;
use std::hash::Hash;

/// Defines an interface for a register-like actor.
#[derive(
    Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
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

impl<RequestId, Value, InternalMsg> RegisterMsg<RequestId, Value, InternalMsg> {
    /// This is a helper for configuring an [`ActorModel`] parameterized by a [`ConsistencyTester`]
    /// for its history. Simply pass this method to [`ActorModel::record_msg_out`]. Records
    /// [`RegisterOp::Read`] upon [`RegisterMsg::Get`] and [`RegisterOp::Write`] upon
    /// [`RegisterMsg::Put`].
    pub fn record_invocations<C, H>(
        _cfg: &C,
        history: &H,
        env: Envelope<&RegisterMsg<RequestId, Value, InternalMsg>>,
    ) -> Option<H>
    where
        H: Clone + ConsistencyTester<Id, Register<Value>>,
        Value: Clone + Debug + PartialEq,
    {
        // Currently throws away useful information about invalid histories. Ideally
        // checking would continue, but the property would be labeled with an error.
        if let Get(_) = env.msg {
            let mut history = history.clone();
            let _ = history.on_invoke(env.src, RegisterOp::Read);
            Some(history)
        } else if let Put(_req_id, value) = env.msg {
            let mut history = history.clone();
            let _ = history.on_invoke(env.src, RegisterOp::Write(value.clone()));
            Some(history)
        } else {
            None
        }
    }

    /// This is a helper for configuring an [`ActorModel`] parameterized by a [`ConsistencyTester`]
    /// for its history. Simply pass this method to [`ActorModel::record_msg_in`]. Records
    /// [`RegisterRet::ReadOk`] upon [`RegisterMsg::GetOk`] and [`RegisterRet::WriteOk`] upon
    /// [`RegisterMsg::PutOk`].
    pub fn record_returns<C, H>(
        _cfg: &C,
        history: &H,
        env: Envelope<&RegisterMsg<RequestId, Value, InternalMsg>>,
    ) -> Option<H>
    where
        H: Clone + ConsistencyTester<Id, Register<Value>>,
        Value: Clone + Debug + PartialEq,
    {
        // Currently throws away useful information about invalid histories. Ideally
        // checking would continue, but the property would be labeled with an error.
        match env.msg {
            GetOk(_, v) => {
                let mut history = history.clone();
                let _ = history.on_return(env.dst, RegisterRet::ReadOk(v.clone()));
                Some(history)
            }
            PutOk(_) => {
                let mut history = history.clone();
                let _ = history.on_return(env.dst, RegisterRet::WriteOk);
                Some(history)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterActor<ServerActor> {
    /// A client that [`RegisterMsg::Put`]s a message and upon receiving a
    /// corresponding [`RegisterMsg::PutOk`] follows up with a
    /// [`RegisterMsg::Get`].
    Client {
        put_count: usize,
        server_count: usize,
    },
    /// A server actor being validated.
    Server(ServerActor),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub enum RegisterActorState<ServerState, RequestId> {
    /// A client that sends a sequence of [`RegisterMsg::Put`] messages before sending a
    /// [`RegisterMsg::Get`].
    Client {
        awaiting: Option<RequestId>,
        op_count: u64,
    },
    /// Wraps the state of a server actor.
    Server(ServerState),
}

// This implementation assumes the servers are at the beginning of the list of
// actors in the system under test so that an arbitrary server destination ID
// can be derived from `(client_id.0 + k) % server_count` for any `k`.
impl<ServerActor, InternalMsg> Actor for RegisterActor<ServerActor>
where
    ServerActor: Actor<Msg = RegisterMsg<u64, char, InternalMsg>>,
    InternalMsg: Clone + Debug + Eq + Hash,
{
    type Msg = RegisterMsg<u64, char, InternalMsg>;
    type State = RegisterActorState<ServerActor::State, u64>;
    type Timer = ServerActor::Timer;
    type Random = ServerActor::Random;
    type Storage = ServerActor::Storage;

    fn name(&self) -> String {
        match self {
            RegisterActor::Client { .. } => "Client".to_owned(),
            RegisterActor::Server(s) => {
                let n = s.name();
                if n.is_empty() {
                    "Server".to_owned()
                } else {
                    n
                }
            }
        }
    }

    #[allow(clippy::identity_op)]
    fn on_start(&self, id: Id, storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        match self {
            RegisterActor::Client {
                put_count,
                server_count,
            } => {
                let server_count = *server_count as u64;

                let index = id.0;
                if index < server_count {
                    panic!("RegisterActor clients must be added to the model after servers.");
                }

                if *put_count == 0 {
                    RegisterActorState::Client {
                        awaiting: None,
                        op_count: 0,
                    }
                } else {
                    let unique_request_id = 1 * index; // next will be 2 * index
                    let value = (b'A' + (index - server_count) as u8) as char;
                    o.send(
                        Id((index + 0) % server_count),
                        Put(unique_request_id, value),
                    );
                    RegisterActorState::Client {
                        awaiting: Some(unique_request_id),
                        op_count: 1,
                    }
                }
            }
            RegisterActor::Server(server_actor) => {
                let mut server_out = Out::new();
                let state =
                    RegisterActorState::Server(server_actor.on_start(id, storage, &mut server_out));
                o.append(&mut server_out);
                state
            }
        }
    }

    fn on_msg(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        use RegisterActor as A;
        use RegisterActorState as S;

        match (self, &**state) {
            (
                A::Client {
                    put_count,
                    server_count,
                },
                S::Client {
                    awaiting: Some(awaiting),
                    op_count,
                },
            ) => {
                let server_count = *server_count as u64;
                match msg {
                    RegisterMsg::PutOk(request_id) if &request_id == awaiting => {
                        let index = id.0;
                        let unique_request_id = (op_count + 1) * index;
                        if *op_count < *put_count as u64 {
                            let value = (b'Z' - (index - server_count) as u8) as char;
                            o.send(
                                Id((index + op_count) % server_count),
                                Put(unique_request_id, value),
                            );
                        } else {
                            o.send(
                                Id((index + op_count) % server_count),
                                Get(unique_request_id),
                            );
                        }
                        *state = Cow::Owned(RegisterActorState::Client {
                            awaiting: Some(unique_request_id),
                            op_count: op_count + 1,
                        });
                    }
                    RegisterMsg::GetOk(request_id, _value) if &request_id == awaiting => {
                        *state = Cow::Owned(RegisterActorState::Client {
                            awaiting: None,
                            op_count: op_count + 1,
                        });
                    }
                    _ => {}
                }
            }
            (A::Server(server_actor), S::Server(server_state)) => {
                let mut server_state = Cow::Borrowed(server_state);
                let mut server_out = Out::new();
                server_actor.on_msg(id, &mut server_state, src, msg, &mut server_out);
                if let Cow::Owned(server_state) = server_state {
                    *state = Cow::Owned(RegisterActorState::Server(server_state))
                }
                o.append(&mut server_out);
            }
            _ => {}
        }
    }

    fn on_timeout(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        use RegisterActor as A;
        use RegisterActorState as S;
        match (self, &**state) {
            (A::Client { .. }, S::Client { .. }) => {}
            (A::Server(server_actor), S::Server(server_state)) => {
                let mut server_state = Cow::Borrowed(server_state);
                let mut server_out = Out::new();
                server_actor.on_timeout(id, &mut server_state, timer, &mut server_out);
                if let Cow::Owned(server_state) = server_state {
                    *state = Cow::Owned(RegisterActorState::Server(server_state))
                }
                o.append(&mut server_out);
            }
            _ => {}
        }
    }
}
