//! This module provides an actor abstraction. See the `system` submodule for a state machine
//! implementation that can check a system of actors. See the `spawn` submodule for a runtime that
//! can run your actor over a real network. See the `register` submodule for an example wrapper.
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
//! struct LogicalClock { bootstrap_to_id: Option<Id> }
//! #[derive(Clone, Debug, Eq, Hash, PartialEq)]
//! struct Timestamp(u32);
//! #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
//! struct MsgWithTimestamp(u32);
//!
//! impl Actor for LogicalClock {
//!     type Msg = MsgWithTimestamp;
//!     type State = Timestamp;
//!
//!     fn init(i: InitIn<Self>, o: &mut Out<Self>) {
//!         if let Some(peer_id) = i.context.bootstrap_to_id {
//!             o.set_state(Timestamp(1));
//!             o.send(peer_id, MsgWithTimestamp(1));
//!         } else {
//!             o.set_state(Timestamp(0));
//!         }
//!     }
//!
//!     fn next(i: NextIn<Self>, o: &mut Out<Self>) {
//!         let Event::Receive(src, MsgWithTimestamp(timestamp)) = i.event;
//!         if timestamp > i.state.0 {
//!             o.set_state(Timestamp(timestamp + 1));
//!             o.send(src, MsgWithTimestamp(timestamp + 1));
//!         }
//!     }
//! }
//!
//! let counterexample = Model {
//!     state_machine: System::with_actors(vec![
//!         LogicalClock { bootstrap_to_id: None},
//!         LogicalClock { bootstrap_to_id: Some(Id::from(0)) }
//!     ]),
//!     properties: vec![Property::always("less than 3", |_: &System<LogicalClock>, state: &SystemState<LogicalClock>| {
//!         state.actor_states.iter().all(|s| s.0 < 3)
//!     })],
//!     boundary: None,
//! }.checker().check(1_000).counterexample("less than 3").unwrap();
//! assert_eq!(
//!     counterexample.last_state().actor_states,
//!     vec![Arc::new(Timestamp(2)), Arc::new(Timestamp(3))]);
//! assert_eq!(
//!     counterexample.into_actions(),
//!     vec![
//!         SystemAction::Act(Id::from(0), Event::Receive(Id::from(1), MsgWithTimestamp(1))),
//!         SystemAction::Act(Id::from(1), Event::Receive(Id::from(0), MsgWithTimestamp(2)))]);
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

/// Uniquely identifies an `Actor`. Encodes the socket address for spawned
/// actors. Encodes an index for model checked actors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Id(u64);

/// Events to which an actor can respond.
#[derive(Debug, PartialEq)]
pub enum Event<Msg> {
    /// Received a message from a sender.
    Receive(Id, Msg),
}

/// Groups inputs to make function types more concise when implementing an actor.
pub struct InitIn<'a, A: Actor> {
    /// Immutable information specific to this actor.
    pub context: &'a A,
    /// A unique identifier for the actor.
    pub id: Id,
}

/// Groups inputs to make function types more concise when implementing an actor.
pub struct NextIn<'a, A: Actor> {
    /// Immutable information specific to this actor.
    pub context: &'a A,
    /// A unique identifier for the actor.
    pub id: Id,
    /// The previous state.
    pub state: &'a A::State,
    /// The event to which the actor is responding.
    pub event: Event<A::Msg>,
}

/// Commands with which an actor can respond.
#[derive(Debug)]
pub enum Command<Msg> {
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
    where
        A::Msg: Clone,
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
    type Msg;

    /// The type of state maintained by the actor.
    type State;

    /// Indicates the initial state and commands.
    fn init(i: InitIn<Self>, o: &mut Out<Self>);

    /// Indicates the next state and commands.
    fn next(i: NextIn<Self>, o: &mut Out<Self>);

    /// Returns the initial state and commands.
    fn init_out(&self, id: Id) -> Out<Self> {
        let i = InitIn { id, context: self };
        let mut o = Out {
            state: None,
            commands: Vec::new(),
        };
        Self::init(i, &mut o);
        o
    }

    /// Returns the next state and commands.
    fn next_out(&self, id: Id, state: &Self::State, event: Event<Self::Msg>) -> Out<Self> {
        let i = NextIn {
            id,
            context: self,
            state,
            event,
        };
        let mut o = Out {
            state: None,
            commands: Vec::new(),
        };
        Self::next(i, &mut o);
        o
    }

    /// Indicates how to deserialize messages received by a spawned actor.
    fn deserialize(bytes: &[u8]) -> serde_json::Result<Self::Msg>
    where
        Self::Msg: DeserializeOwned,
    {
        serde_json::from_slice(bytes)
    }

    /// Indicates how to serialize messages sent by a spawned actor.
    fn serialize(msg: &Self::Msg) -> serde_json::Result<Vec<u8>>
    where
        Self::Msg: Serialize,
    {
        serde_json::to_vec(msg)
    }
}
