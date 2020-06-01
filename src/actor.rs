//! This module provides an actor abstraction. See the `system` submodule for model checking.
//! See the `spawn` submodule for a runtime that can run your actor over a real network. See the
//! `register` submodule for an example wrapper.
//!
//! ## Example
//!
//! In the following example two actors track events with logical clocks. A false claim is made
//! that a clock will never reach 3, which the checker disproves by demonstrating that the actors
//! continue sending messages back and forth (therefore increasing their clocks) after an initial
//! message is sent.
//!
//! ```
//! use stateright::*;
//! use stateright::actor::*;
//! use stateright::actor::system::*;
//! use stateright::checker::*;
//! use std::collections::BTreeSet;
//! use std::iter::FromIterator;
//! use std::sync::Arc;
//!
//! /// The actor needs to know whether it should "bootstrap" by sending the first
//! /// message. If so, it needs to know to which peer the message should be sent.
//! struct LogicalClockActor { bootstrap_to_id: Option<Id> }
//!
//! /// Actor state is simply a "timestamp" sequencer.
//! #[derive(Clone, Debug, Eq, Hash, PartialEq)]
//! struct Timestamp(u32);
//!
//! /// And we define a generic message containing a timestamp.
//! #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
//! struct MsgWithTimestamp(u32);
//!
//! impl Actor for LogicalClockActor {
//!     type Msg = MsgWithTimestamp;
//!     type State = Timestamp;
//!
//!     fn on_start(&self, _id: Id, o: &mut Out<Self>) {
//!         // The actor either bootstraps or starts at time zero.
//!         if let Some(peer_id) = self.bootstrap_to_id {
//!             o.set_state(Timestamp(1));
//!             o.send(peer_id, MsgWithTimestamp(1));
//!         } else {
//!             o.set_state(Timestamp(0));
//!         }
//!     }
//!
//!     fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
//!         // Upon receiving a message, the actor updates its timestamp and replies.
//!         let MsgWithTimestamp(timestamp) = msg;
//!         if timestamp > state.0 {
//!             o.set_state(Timestamp(timestamp + 1));
//!             o.send(src, MsgWithTimestamp(timestamp + 1));
//!         }
//!     }
//! }
//!
//! /// We now define the actor system, which we parameterize by the maximum
//! /// expected timestamp.
//! struct LogicalClockSystem { max_expected: u32 };
//!
//! impl System for LogicalClockSystem {
//!     type Actor = LogicalClockActor;
//!
//!     /// The system contains two actors, one of which bootstraps.
//!     fn actors(&self) -> Vec<Self::Actor> {
//!         vec![
//!             LogicalClockActor { bootstrap_to_id: None},
//!             LogicalClockActor { bootstrap_to_id: Some(Id::from(0)) }
//!         ]
//!     }
//!
//!     /// The only property is one indicating that every actor's timestamp is less than the
//!     /// maximum expected timestamp defined for the system.
//!     fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
//!         vec![Property::<SystemModel<Self>>::always("less than max", |model, state| {
//!             state.actor_states.iter().all(|s| s.0 < model.system.max_expected)
//!         })]
//!     }
//! }
//!
//! // The model checker should quickly find a counterexample sequence of actions that causes an
//! // actor timestamp to reach a specified maximum.
//! let counterexample = LogicalClockSystem { max_expected: 3 }
//!     .into_model().checker().check(1_000)
//!     .counterexample("less than max").unwrap();
//! assert_eq!(
//!     counterexample.last_state().actor_states,
//!     vec![Arc::new(Timestamp(2)), Arc::new(Timestamp(3))]);
//! assert_eq!(
//!     counterexample.into_actions(),
//!     vec![
//!         SystemAction::Deliver { src: Id::from(1), dst: Id::from(0), msg: MsgWithTimestamp(1) },
//!         SystemAction::Deliver { src: Id::from(0), dst: Id::from(1), msg: MsgWithTimestamp(2) }]);
//! ```
//!
//! [Additional examples](https://github.com/stateright/stateright/tree/master/examples)
//! are available in the repository.

pub mod register;
pub mod spawn;
pub mod system;

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::hash::Hash;
use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;
use std::net::SocketAddrV4;
use std::ops::Range;

/// Uniquely identifies an `Actor`. Encodes the socket address for spawned
/// actors. Encodes an index for model checked actors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Id(u64);

impl Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&SocketAddrV4::from(*self), f)
    }
}

/// Commands with which an actor can respond.
#[derive(Debug)]
pub enum Command<Msg> {
    /// Cancel the timer if one is set.
    CancelTimer,
    /// Set/reset the timer.
    SetTimer(Range<Duration>),
    /// Send a message to a destination.
    Send(Id, Msg),
}

/// Groups outputs to make function types more concise when implementing an actor.
pub struct Out<A: Actor> {
    /// The new actor state. `None` indicates no change.
    pub state: Option<A::State>,
    /// Commands output by the actor.
    pub commands: Vec<Command<A::Msg>>,
}

impl<A: Actor> Out<A> {
    /// Records the need to set the timer.
    pub fn set_timer(&mut self, duration: Range<Duration>) {
        self.commands.push(Command::SetTimer(duration));
    }

    /// Records the need to cancel the timer.
    pub fn cancel_timer(&mut self) {
        self.commands.push(Command::CancelTimer);
    }

    /// Updates the actor state.
    pub fn set_state(&mut self, state: A::State) {
        self.state = Some(state);
    }

    /// Records the need to send a message.
    pub fn send(&mut self, recipient: Id, msg: A::Msg) {
        self.commands.push(Command::Send(recipient, msg));
    }

    /// Records the need to send a message to multiple recipient.
    pub fn broadcast(&mut self, recipients: &[Id], msg: &A::Msg)
    where A::Msg: Clone
    {
        for recipient in recipients {
            self.send(*recipient, msg.clone());
        }
    }
}

/// An actor initializes internal state optionally emitting outputs; then it waits for incoming
/// events, responding by updating its internal state and optionally emitting outputs.  At the
/// moment, the only inputs and outputs relate to messages, but other events like timers will
/// likely be added.
pub trait Actor: Sized {
    /// The type of messages sent and received by this actor.
    type Msg: Clone + Debug + Ord;

    /// The type of state maintained by the actor.
    type State: Clone + Debug;

    /// Indicates the initial state and commands.
    fn on_start(&self, id: Id, o: &mut Out<Self>);

    /// Indicates the next state and commands when a message is received.
    fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>);

    /// Indicates the next state and commands when a timeout is encountered.
    fn on_timeout(&self, _id: Id, _state: &Self::State, _o: &mut Out<Self>) {
        // no-op by default
    }

    /// Returns the initial state and commands.
    fn on_start_out(&self, id: Id) -> Out<Self> {
        let mut o = Out {
            state: None,
            commands: Vec::new(),
        };
        self.on_start(id, &mut o);
        o
    }

    /// Returns the next state and commands when a message is received.
    fn on_msg_out(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg) -> Out<Self> {
        let mut o = Out {
            state: None,
            commands: Vec::new(),
        };
        self.on_msg(id, state, src, msg, &mut o);
        o
    }

    /// Returns the next state and commands when a timeout is encountered.
    fn on_timeout_out(&self, id: Id, state: &Self::State) -> Out<Self> {
        let mut o = Out {
            state: None,
            commands: Vec::new(),
        };
        self.on_timeout(id, state, &mut o);
        o
    }

    /// Indicates how to deserialize messages received by a spawned actor.
    fn deserialize(bytes: &[u8]) -> serde_json::Result<Self::Msg> where Self::Msg: DeserializeOwned {
        serde_json::from_slice(bytes)
    }

    /// Indicates how to serialize messages sent by a spawned actor.
    fn serialize(msg: &Self::Msg) -> serde_json::Result<Vec<u8>> where Self::Msg: Serialize {
        serde_json::to_vec(msg)
    }
}
