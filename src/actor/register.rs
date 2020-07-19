//! Defines an interface for register-like actors (via [`RegisterMsg`]) and also provides
//! [`RegisterTestSystem`] for model checking.
//!
//! [`RegisterTestSystem`]: struct.RegisterTestSystem.html
//! [`RegisterMsg`]: enum.RegisterMsg.html

use crate::Property;
use crate::actor::{Actor, Id};
use crate::actor::system::{Envelope, System, SystemModel, SystemState, LossyNetwork};
use crate::util::HashableHashMap;
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
    GetOk(RequestId, Option<Value>),
}
use RegisterMsg::*;

/// A system for testing an actor service with register semantics.
#[derive(Clone)]
pub struct RegisterTestSystem<A, I>
where
    A: Actor<Msg = RegisterMsg<TestRequestId, TestValue, I>> + Clone,
    I: Clone + Debug + Eq + Hash,
{
    pub servers: Vec<A>,
    pub put_count: u8,
    pub get_count: u8,
    pub within_boundary: fn(state: &SystemState<Self>) -> bool,
    pub lossy_network: LossyNetwork,
}

impl<A, I> Default for RegisterTestSystem<A, I>
    where
    A: Actor<Msg = RegisterMsg<TestRequestId, TestValue, I>> + Clone,
    I: Clone + Debug + Eq + Hash,
{
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            put_count: 3,
            get_count: 2,
            within_boundary: |_| true,
            lossy_network: LossyNetwork::No,
        }
    }
}

impl<A, I> System for RegisterTestSystem<A, I>
    where
        A: Actor<Msg = RegisterMsg<TestRequestId, TestValue, I>> + Clone,
        I: Clone + Debug + Eq + Hash,
{
    type Actor = A;
    type History = RegisterHistory<TestRequestId, TestValue>;

    fn actors(&self) -> Vec<Self::Actor> {
        self.servers.clone()
    }

    /// The following code sets up the network based on the requested put and get counts. Each
    /// operation has a unique request ID and network source, while puts also have a unique value.
    /// We derive all three from the same unique ID.
    fn init_network(&self) -> Vec<Envelope<<Self::Actor as Actor>::Msg>> {
        let mut unique_id = 0_u8;
        let mut network = Vec::new();
        for dst in 0..self.put_count as usize {
            network.push(Envelope {
                src: Id::from(999 - unique_id as usize),
                dst: Id::from(dst % self.servers.len()),
                msg: Put(unique_id as u64, ('A' as u8 + unique_id) as char),
            });
            unique_id += 1;
        }
        for dst in 0..self.get_count as usize {
            network.push(Envelope {
                src: Id::from(999 - unique_id as usize),
                dst: Id::from(dst % self.servers.len()),
                msg: Get(unique_id as u64),
            });
            unique_id += 1;
        }
        network
    }

    fn lossy_network(&self) -> LossyNetwork {
        self.lossy_network
    }

    fn record_msg_in(&self, history: &Self::History, _src: Id, _dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        if let Put(req_id, value) = msg {
            let mut history = history.clone();
            history.put_history.insert(*req_id, value.clone());
            Some(history)
        } else {
            None
        }
    }

    fn record_msg_out(&self, history: &Self::History, _src: Id, _dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        match msg {
            PutOk(req_id) => {
                let mut history = history.clone();
                history.last_put_req_id = Some(*req_id);
                history.current_get_value = None;
                return Some(history);
            },
            GetOk(_req_id, value) => {
                let mut history = history.clone();
                history.current_get_value = Some(value.clone());
                return Some(history);
            },
            _ => if history.current_get_value.is_some() {
                let mut history = history.clone();
                history.current_get_value = None;
                return Some(history);
            }
        }
        None
    }

    fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
        vec![
            // This is an overly strict definition of linearizability. Namely, it requires that a
            // read cannot observe an in-flight write; instead the write will always complete
            // before the in-flight read is processed. This requirement simplifies history tracking
            // and ensures linearizability, but a more sophisticated history would allow this
            // property to accept a wider variety of linearizable behaviors.
            Property::<SystemModel<Self>>::always("linearizable", |_model, state| {
                match (state.history.last_put_req_id, state.history.current_get_value) {
                    // Expect no value until put. An equally valid definition would be that the
                    // value is undefined until put, but in practice that's an unlikely
                    // implementation.
                    (None, Some(observed_value)) => observed_value.is_none(),
                    // Can't disprove anything without a get.
                    (_, None) => true,
                    // Otherwise the current get needs to match the last acknowledged put.
                    (Some(req_id), Some(observed_value)) => {
                        match state.history.put_history.get(&req_id) {
                            None => false, // indicates service replied to a put it never received
                            Some(expected_value) => {
                                observed_value.as_ref() == Some(expected_value)
                            }
                        }
                    }
                }
            }),
            Property::<SystemModel<Self>>::sometimes("value chosen",  |_, state| {
                for env in &state.network {
                    if let RegisterMsg::GetOk(_req_id, Some(_value)) = env.msg {
                        return true;
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

/// A simple request ID type for tests.
pub type TestRequestId = u64;

/// A simple value type for tests.
pub type TestValue = char;

/// Captures necessary history for validating register properties.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct RegisterHistory<RequestId: Eq + Hash, Value: Eq + Hash> {
    /// All incoming puts, regardless of whether they completed.
    put_history: HashableHashMap<RequestId, Value>,
    /// Request ID of the last successfully completed put.
    last_put_req_id: Option<RequestId>,
    /// Value of a get that just successfully completed. `None` indicates no recent get, while
    /// `Some(None)` indicates an empty get.
    current_get_value: Option<Option<Value>>,
}
