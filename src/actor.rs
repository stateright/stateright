//! This module provides an [Actor] trait, which can be model checked using [`ActorModel`].  You
//! can also [`spawn()`] the actor in which case it will communicate over a UDP socket.
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
//! use std::borrow::Cow;
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
//! #[derive(Clone, Debug, Eq, Hash, PartialEq)]
//! struct MsgWithTimestamp(u32);
//!
//! impl Actor for LogicalClockActor {
//!     type Msg = MsgWithTimestamp;
//!     type State = Timestamp;
//!
//!     fn on_start(&self, _id: Id, o: &mut Out<Self>) -> Self::State {
//!         // The actor either bootstraps or starts at time zero.
//!         if let Some(peer_id) = self.bootstrap_to_id {
//!             o.send(peer_id, MsgWithTimestamp(1));
//!             Timestamp(1)
//!         } else {
//!             Timestamp(0)
//!         }
//!     }
//!
//!     fn on_msg(&self, id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
//!         // Upon receiving a message, the actor updates its timestamp and replies.
//!         let MsgWithTimestamp(timestamp) = msg;
//!         if timestamp > state.0 {
//!             o.send(src, MsgWithTimestamp(timestamp + 1));
//!             *state.to_mut() = Timestamp(timestamp + 1);
//!         }
//!     }
//! }
//!
//! // The model checker should quickly find a counterexample sequence of actions that causes an
//! // actor timestamp to reach a specified maximum.
//! let checker = ActorModel::new((), ())
//!     .actor(LogicalClockActor { bootstrap_to_id: None})
//!     .actor(LogicalClockActor { bootstrap_to_id: Some(Id::from(0)) })
//!     .property(Expectation::Always, "less than max", |_, state| {
//!         state.actor_states.iter().all(|s| s.0 < 3)
//!     })
//!     .checker().spawn_bfs().join();
//! checker.assert_discovery("less than max", vec![
//!     ActorModelAction::Deliver {
//!         src: Id::from(1),
//!         dst: Id::from(0),
//!         msg: MsgWithTimestamp(1),
//!     },
//!     ActorModelAction::Deliver {
//!         src: Id::from(0),
//!         dst: Id::from(1),
//!         msg: MsgWithTimestamp(2),
//!     },
//! ]);
//! assert_eq!(
//!     checker.discovery("less than max").unwrap().last_state().actor_states,
//!     vec![Arc::new(Timestamp(2)), Arc::new(Timestamp(3))]);
//! ```
//!
//! [Additional examples](https://github.com/stateright/stateright/tree/master/examples)
//! are available in the repository.

use choice::{Choice, Never};
mod model;
mod model_state;
mod spawn;
use std::borrow::Cow;
use std::hash::Hash;
use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;
use std::net::SocketAddrV4;
use std::ops::Range;

#[cfg(test)]
pub mod actor_test_util;
pub use model::*;
pub use model_state::*;
pub mod ordered_reliable_link;
pub mod register;
pub use spawn::*;

/// Uniquely identifies an [`Actor`]. Encodes the socket address for spawned
/// actors. Encodes an index for model checked actors.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Id(u64);

impl Debug for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // KLUDGE: work around the issue identified in https://github.com/rust-lang/rfcs/pull/1198
        //         by not conveying that `Id` is a struct.
        f.write_fmt(format_args!("Id({})", self.0))
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&SocketAddrV4::from(*self), f)
    }
}

impl Id {
    /// Generates a [`Vec`] of [`Id`]s based on an iterator.
    ///
    /// # Example
    ///
    /// ```
    /// use stateright::actor::Id;
    /// let ids = Id::vec_from(0..3);
    /// ```
    pub fn vec_from<T>(ids: impl IntoIterator<Item=T>) -> Vec<Id>
    where T: Into<Id>
    {
        ids.into_iter().map(Into::into).collect()
    }
}

impl From<Id> for usize {
    fn from(id: Id) -> Self {
        id.0 as usize
    }
}

impl From<usize> for Id {
    fn from(u: usize) -> Self {
        Id(u as u64)
    }
}

/// Commands with which an actor can respond.
#[derive(Debug)]
#[derive(serde::Serialize)]
pub enum Command<Msg> {
    /// Cancel the timer if one is set.
    CancelTimer,
    /// Set/reset the timer.
    SetTimer(Range<Duration>),
    /// Send a message to a destination.
    Send(Id, Msg),
}

/// Holds [`Command`]s output by an actor.
pub struct Out<A: Actor>(Vec<Command<A::Msg>>);

impl<A: Actor> Out<A> {
    /// Constructs an empty `Out`.
    fn new() -> Self {
        Self(Vec::new())
    }

    /// Moves all [`Command`]s of `other` into `Self`, leaving `other` empty.
    fn append<B>(&mut self, other: &mut Out<B>)
    where B: Actor<Msg = A::Msg>
    {
        self.0.append(&mut other.0)
    }

    /// Records the need to set the timer. See [`Actor::on_timeout`].
    pub fn set_timer(&mut self, duration: Range<Duration>) {
        self.0.push(Command::SetTimer(duration));
    }

    /// Records the need to cancel the timer.
    pub fn cancel_timer(&mut self) {
        self.0.push(Command::CancelTimer);
    }

    /// Records the need to send a message. See [`Actor::on_msg`].
    pub fn send(&mut self, recipient: Id, msg: A::Msg) {
        self.0.push(Command::Send(recipient, msg));
    }

    /// Records the need to send a message to multiple recipients. See [`Actor::on_msg`].
    pub fn broadcast<'a>(&mut self, recipients: impl IntoIterator<Item=&'a Id>, msg: &A::Msg)
    where A::Msg: Clone
    {
        for recipient in recipients {
            self.send(*recipient, msg.clone());
        }
    }
}

impl<A: Actor> Debug for Out<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<A: Actor> std::ops::Deref for Out<A> {
    type Target = [Command<A::Msg>];
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<A: Actor> std::iter::FromIterator<Command<A::Msg>> for Out<A> {
    fn from_iter<I: IntoIterator<Item = Command<A::Msg>>>(iter: I) -> Self {
        Out(Vec::from_iter(iter))
    }
}

impl<A: Actor> IntoIterator for Out<A> {
    type Item = Command<A::Msg>;
    type IntoIter = std::vec::IntoIter<Command<A::Msg>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// If true, then the actor did not update its state or output commands.
#[allow(clippy::ptr_arg)] // `&Cow` needed for `matches!`
pub fn is_no_op<A: Actor>(state: &Cow<A::State>, out: &Out<A>) -> bool {
    matches!(state, Cow::Borrowed(_)) && out.0.is_empty()
}

/// An actor initializes internal state optionally emitting [outputs]; then it waits for incoming
/// events, responding by updating its internal state and optionally emitting [outputs].
///
/// [outputs]: Out
pub trait Actor: Sized {
    /// The type of messages sent and received by the actor.
    ///
    /// # Example
    ///
    /// ```
    /// use serde::{Deserialize, Serialize};
    /// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    /// #[derive(Serialize, Deserialize)]
    /// enum MyActorMsg { Msg1(u64), Msg2(char) }
    /// ```
    type Msg: Clone + Debug + Eq + Hash;

    /// The type of state maintained by the actor.
    ///
    /// # Example
    ///
    /// ```
    /// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    /// struct MyActorState { sequencer: u64 }
    /// ```
    type State: Clone + Debug + PartialEq + Hash;

    /// Indicates the initial state and commands.
    fn on_start(&self, id: Id, o: &mut Out<Self>) -> Self::State;

    /// Indicates the next state and commands when a message is received. See [`Out::send`].
    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>);

    /// Indicates the next state and commands when a timeout is encountered. See [`Out::set_timer`].
    fn on_timeout(&self, _id: Id, _state: &mut Cow<Self::State>, _o: &mut Out<Self>) {
        // no-op by default
    }
}

impl<A> Actor for Choice<A, Never> where A: Actor {
    type Msg = A::Msg;
    type State = Choice<A::State, Never>;

    fn on_start(&self, id: Id, o: &mut Out<Self>) -> Self::State {
        let actor = self.get();
        let mut o_prime = Out::new();
        let state = actor.on_start(id, &mut o_prime);

        o.append(&mut o_prime);
        Choice::new(state)
    }

    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>,
              src: Id, msg: Self::Msg, o: &mut Out<Self>)
    {
        let actor = self.get();
        let mut state_prime = Cow::Borrowed(state.get());
        let mut o_prime = Out::new();
        actor.on_msg(id, &mut state_prime, src, msg, &mut o_prime);

        o.append(&mut o_prime);
        if let Cow::Owned(state_prime) = state_prime {
            *state = Cow::Owned(Choice::new(state_prime))
        }
    }

    fn on_timeout(&self, id: Id, state: &mut Cow<Self::State>, o: &mut Out<Self>) {
        let actor = self.get();
        let mut state_prime = Cow::Borrowed(state.get());
        let mut o_prime = Out::new();
        actor.on_timeout(id, &mut state_prime, &mut o_prime);

        o.append(&mut o_prime);
        if let Cow::Owned(state_prime) = state_prime {
            *state = Cow::Owned(Choice::new(state_prime));
        }
    }
}

impl<Msg, A1, A2> Actor for Choice<A1, A2>
where Msg: Clone + Debug + Eq + Hash,
      A1: Actor<Msg = Msg>,
      A2: Actor<Msg = Msg>,
{
    type Msg = Msg;
    type State = Choice<A1::State, A2::State>;

    fn on_start(&self, id: Id, o: &mut Out<Self>) -> Self::State {
        match self {
            Choice::L(actor) => {
                let mut o_prime = Out::new();
                let state = actor.on_start(id, &mut o_prime);
                o.append(&mut o_prime);
                Choice::L(state)
            }
            Choice::R(actor) => {
                let mut o_prime = Out::new();
                let state = actor.on_start(id, &mut o_prime);
                o.append(&mut o_prime);
                Choice::R(state)
            }
        }
    }

    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>,
              src: Id, msg: Self::Msg, o: &mut Out<Self>)
    {
        match (self, &**state) {
            (Choice::L(actor), Choice::L(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_msg(id, &mut state_prime, src, msg, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::L(state_prime));
                }
            }
            (Choice::R(actor), Choice::R(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_msg(id, &mut state_prime, src, msg, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::R(state_prime));
                }
            }
            _ => unreachable!(),
        }
    }

    fn on_timeout(&self, id: Id, state: &mut Cow<Self::State>, o: &mut Out<Self>) {
        match (self, &**state) {
            (Choice::L(actor), Choice::L(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_timeout(id, &mut state_prime, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::L(state_prime));
                }
            }
            (Choice::R(actor), Choice::R(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_timeout(id, &mut state_prime, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::R(state_prime));
                }
            }
            _ => unreachable!(),
        }
    }
}

/// Implemented only for rustdoc tests. Do not take a dependency on this. It will likely be removed
/// in a future version of this library.
#[doc(hidden)]
impl Actor for () {
    type State = ();
    type Msg = ();
    fn on_start(&self, _: Id, _o: &mut Out<Self>) -> Self::State {}
    fn on_msg(&self, _: Id, _: &mut Cow<Self::State>, _: Id, _: Self::Msg, _: &mut Out<Self>) {}
}

/// Indicates the number of nodes that constitute a majority for a particular cluster size.
pub fn majority(cluster_size: usize) -> usize {
    cluster_size / 2 + 1
}

/// A helper to generate peer [`Id`]s.
pub fn peer_ids<'a, Id: PartialEq + 'static>(self_id: Id, other_ids: impl IntoIterator<Item=&'a Id>) -> impl Iterator<Item=&'a Id> {
    other_ids.into_iter().filter(move |o_id| o_id != &&self_id)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn majority_is_computed_correctly() {
        assert_eq!(majority(1), 1);
        assert_eq!(majority(2), 2);
        assert_eq!(majority(3), 2);
        assert_eq!(majority(4), 3);
        assert_eq!(majority(5), 3);
    }

    #[test]
    fn peer_ids_are_computed_correctly() {
        let ids: Vec<Id> = (0..3).into_iter().map(Id::from).collect();
        assert_eq!(
            peer_ids(ids[1], &ids).collect::<Vec<&Id>>(),
            vec![&Id::from(0), &Id::from(2)]);
    }
}
