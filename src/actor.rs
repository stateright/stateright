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
//!     type Timer = ();
//!     type Random = ();
//!     type Storage = ();
//!
//!     fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
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
mod network;
mod spawn;
mod timers;
use std::borrow::Cow;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::net::SocketAddrV4;
use std::ops::Range;
use std::time::Duration;

#[cfg(test)]
pub mod actor_test_util;
pub use model::*;
pub use model_state::*;
pub use network::*;
pub use timers::*;
pub mod ordered_reliable_link;
pub mod register;
pub mod write_once_register;
pub use spawn::*;

/// Uniquely identifies an [`Actor`]. Encodes the socket address for spawned
/// actors. Encodes an index for model checked actors.
#[derive(
    Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
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
    pub fn vec_from<T>(ids: impl IntoIterator<Item = T>) -> Vec<Id>
    where
        T: Into<Id>,
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
#[derive(Debug, serde::Serialize)]
pub enum Command<Msg, Timer, Random, Storage> {
    /// Cancel the timer if one is set.
    CancelTimer(Timer),
    /// Set/reset the timer.
    SetTimer(Timer, Range<Duration>),
    /// Send a message to a destination.
    Send(Id, Msg),
    /// Choose a random.
    ChooseRandom(String, Vec<Random>),
    /// Save non-volatile state.
    Save(Storage),
}

/// Holds [`Command`]s output by an actor.
pub struct Out<A: Actor>(Vec<Command<A::Msg, A::Timer, A::Random, A::Storage>>);

impl<A: Actor> Default for Out<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Actor> Out<A> {
    /// Constructs an empty `Out`.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Moves all [`Command`]s of `other` into `Self`, leaving `other` empty.
    pub fn append<B>(&mut self, other: &mut Out<B>)
    where
        B: Actor<Msg = A::Msg, Timer = A::Timer, Random = A::Random, Storage = A::Storage>,
    {
        self.0.append(&mut other.0)
    }

    /// Records the need to set the timer. See [`Actor::on_timeout`].
    pub fn set_timer(&mut self, timer: A::Timer, duration: Range<Duration>) {
        self.0.push(Command::SetTimer(timer, duration));
    }

    /// Records the need to cancel the timer.
    pub fn cancel_timer(&mut self, timer: A::Timer) {
        self.0.push(Command::CancelTimer(timer));
    }

    /// Records the need to send a message. See [`Actor::on_msg`].
    pub fn send(&mut self, recipient: Id, msg: A::Msg) {
        self.0.push(Command::Send(recipient, msg));
    }

    /// Records the need to send a message to multiple recipients. See [`Actor::on_msg`].
    pub fn broadcast<'a>(&mut self, recipients: impl IntoIterator<Item = &'a Id>, msg: &A::Msg)
    where
        A::Msg: Clone,
    {
        for recipient in recipients {
            self.send(*recipient, msg.clone());
        }
    }

    /// Records a non-deterministic choice by the actor.
    ///
    /// Actors can use this method to record a `random` vec of possible choices, creating a branch
    /// in the search tree for the actor. The `key` is a unique identifier for the choice to be
    /// made — `choose_random` will overwrite any previous calls with the same key.
    ///
    /// See [`Actor::on_random`].
    pub fn choose_random(&mut self, key: impl Into<String>, random: Vec<A::Random>) {
        self.0.push(Command::ChooseRandom(key.into(), random));
    }

    /// Records the need to cancel a random choice.
    ///
    /// Removes any previous `choose_random` calls with the same key.
    ///
    /// See [`Actor::on_random`].
    pub fn remove_random(&mut self, key: impl Into<String>) {
        self.0.push(Command::ChooseRandom(key.into(), vec![]));
    }

    /// Records the need to save non-volatile state. See [`Actor::on_start`].
    pub fn save(&mut self, storage: A::Storage) {
        self.0.push(Command::Save(storage));
    }
}

impl<A: Actor> Debug for Out<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<A: Actor> std::ops::Deref for Out<A> {
    type Target = [Command<A::Msg, A::Timer, A::Random, A::Storage>];
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<A: Actor> std::iter::FromIterator<Command<A::Msg, A::Timer, A::Random, A::Storage>>
    for Out<A>
{
    fn from_iter<I: IntoIterator<Item = Command<A::Msg, A::Timer, A::Random, A::Storage>>>(
        iter: I,
    ) -> Self {
        Out(Vec::from_iter(iter))
    }
}

impl<A: Actor> IntoIterator for Out<A> {
    type Item = Command<A::Msg, A::Timer, A::Random, A::Storage>;
    type IntoIter = std::vec::IntoIter<Command<A::Msg, A::Timer, A::Random, A::Storage>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// If true, then the actor did not update its state or output commands.
#[allow(clippy::ptr_arg)] // `&Cow` needed for `matches!`
pub fn is_no_op<A: Actor>(state: &Cow<A::State>, out: &Out<A>) -> bool {
    matches!(state, Cow::Borrowed(_)) && out.0.is_empty()
}

/// If true, then the actor did not update its state or output commands, besides renewing the same
/// timer.
#[allow(clippy::ptr_arg)] // `&Cow` needed for `matches!`
pub fn is_no_op_with_timer<A: Actor>(
    state: &Cow<A::State>,
    out: &Out<A>,
    timer: &A::Timer,
) -> bool {
    let keep_timer = out
        .iter()
        .any(|c| matches!(c, Command::SetTimer(t, _) if t == timer));
    let unmodified_out = out.0.len() == 1 && keep_timer;
    matches!(state, Cow::Borrowed(_)) && unmodified_out
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

    /// The type for tagging timers.
    ///
    /// # Example
    ///
    /// ```
    /// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    /// enum MyActorTimer { Event1, Event2 }
    /// ```
    type Timer: Clone + Debug + Eq + Hash;

    /// The type of state maintained by the actor.
    ///
    /// # Example
    ///
    /// ```
    /// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    /// struct MyActorState { sequencer: u64 }
    /// ```
    type State: Clone + Debug + PartialEq + Hash;

    /// The type of non-volatile state maintained by the server.
    /// This should be a subset of the `State` field.
    ///
    /// # Example
    /// ```
    /// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    /// struct MyActorStorage { sequencer: u64 }
    /// ```
    type Storage: Clone + Debug + PartialEq + Hash;

    /// The type for tagging random choices by an actor.
    ///
    /// # Example
    ///
    /// ```
    /// #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    /// enum MyActorRandom { Choice1, Choice2 }
    /// ```
    type Random: Clone + Debug + Eq + Hash;

    /// Indicates the initial state and commands.
    fn on_start(&self, id: Id, storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State;

    /// Indicates the next state and commands when a message is received. See [`Out::send`].
    fn on_msg(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        // no-op by default
        let _ = id;
        let _ = state;
        let _ = src;
        let _ = msg;
        let _ = o;
    }

    /// Indicates the next state and commands when a timeout is encountered. See [`Out::set_timer`].
    fn on_timeout(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        _timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        // no-op by default
        let _ = id;
        let _ = state;
        let _ = o;
    }

    /// Indicates the next state and commands when a random choice is selected. See
    /// [`Out::choose_random`].
    fn on_random(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        random: &Self::Random,
        o: &mut Out<Self>,
    ) {
        // no-op by default
        let _ = id;
        let _ = state;
        let _ = random;
        let _ = o;
    }

    fn name(&self) -> String {
        String::new()
    }
}

impl<A> Actor for Choice<A, Never>
where
    A: Actor,
{
    type Msg = A::Msg;
    type State = Choice<A::State, Never>;
    type Timer = A::Timer;
    type Random = A::Random;
    type Storage = A::Storage;

    fn on_start(&self, id: Id, storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        let actor = self.get();
        let mut o_prime = Out::new();
        let state = actor.on_start(id, storage, &mut o_prime);
        o.append(&mut o_prime);
        Choice::new(state)
    }

    fn on_msg(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        let actor = self.get();
        let mut state_prime = Cow::Borrowed(state.get());
        let mut o_prime = Out::new();
        actor.on_msg(id, &mut state_prime, src, msg, &mut o_prime);

        o.append(&mut o_prime);
        if let Cow::Owned(state_prime) = state_prime {
            *state = Cow::Owned(Choice::new(state_prime))
        }
    }

    fn on_timeout(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        let actor = self.get();
        let mut state_prime = Cow::Borrowed(state.get());
        let mut o_prime = Out::new();
        actor.on_timeout(id, &mut state_prime, timer, &mut o_prime);

        o.append(&mut o_prime);
        if let Cow::Owned(state_prime) = state_prime {
            *state = Cow::Owned(Choice::new(state_prime));
        }
    }

    fn name(&self) -> String {
        self.get().name()
    }
}

impl<Msg, Timer, Random, Storage, A1, A2> Actor for Choice<A1, A2>
where
    Msg: Clone + Debug + Eq + Hash,
    Timer: Clone + Debug + Eq + Hash + serde::Serialize,
    Random: Clone + Debug + Eq + Hash,
    Storage: Clone + Debug + Eq + Hash + serde::Serialize,
    A1: Actor<Msg = Msg, Timer = Timer, Storage = Storage, Random = Random>,
    A2: Actor<Msg = Msg, Timer = Timer, Storage = Storage, Random = Random>,
{
    type Msg = Msg;
    type State = Choice<A1::State, A2::State>;
    type Timer = Timer;
    type Random = Random;
    type Storage = Storage;
    fn on_start(&self, id: Id, storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        match self {
            Choice::L(actor) => {
                let mut o_prime = Out::new();
                let state = actor.on_start(id, storage, &mut o_prime);
                o.append(&mut o_prime);
                Choice::L(state)
            }
            Choice::R(actor) => {
                let mut o_prime = Out::new();
                let state = actor.on_start(id, storage, &mut o_prime);
                o.append(&mut o_prime);
                Choice::R(state)
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

    fn on_timeout(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        match (self, &**state) {
            (Choice::L(actor), Choice::L(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_timeout(id, &mut state_prime, timer, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::L(state_prime));
                }
            }
            (Choice::R(actor), Choice::R(state_prime)) => {
                let mut state_prime = Cow::Borrowed(state_prime);
                let mut o_prime = Out::new();
                actor.on_timeout(id, &mut state_prime, timer, &mut o_prime);
                o.append(&mut o_prime);
                if let Cow::Owned(state_prime) = state_prime {
                    *state = Cow::Owned(Choice::R(state_prime));
                }
            }
            _ => unreachable!(),
        }
    }

    fn name(&self) -> String {
        match self {
            Choice::L(a) => a.name(),
            Choice::R(a) => a.name(),
        }
    }
}

/// Implemented only for rustdoc tests. Do not take a dependency on this. It will likely be removed
/// in a future version of this library.
#[doc(hidden)]
impl Actor for () {
    type State = ();
    type Msg = ();
    type Timer = ();
    type Random = ();
    type Storage = ();
    fn on_start(&self, _: Id, _storage: &Option<Self::Storage>, _o: &mut Out<Self>) -> Self::State {
    }
    fn on_msg(&self, _: Id, _: &mut Cow<Self::State>, _: Id, _: Self::Msg, _: &mut Out<Self>) {}
    fn name(&self) -> String {
        String::new()
    }
}

/// Sends a series of messages in sequence to the associated actor [`Id`]s waiting for a message
/// delivery between each. This is useful for testing actor systems.
impl<Msg> Actor for Vec<(Id, Msg)>
where
    Msg: Clone + Debug + Eq + Hash,
{
    type Msg = Msg;
    type State = usize;
    type Timer = ();
    type Random = ();
    type Storage = ();
    fn on_start(
        &self,
        _id: Id,
        _storage: &Option<Self::Storage>,
        o: &mut Out<Self>,
    ) -> Self::State {
        if let Some((dst, msg)) = self.first() {
            o.send(*dst, msg.clone());
            1
        } else {
            0
        }
    }

    fn on_msg(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        _src: Id,
        _msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        if let Some((dst, msg)) = self.get(**state) {
            o.send(*dst, msg.clone());
            *state.to_mut() += 1;
        }
    }

    fn name(&self) -> String {
        String::new()
    }
}

/// Indicates the number of nodes that constitute a majority for a particular cluster size.
pub fn majority(cluster_size: usize) -> usize {
    cluster_size / 2 + 1
}

/// A helper to generate peer [`Id`]s.
pub fn peer_ids<'a, Id: PartialEq + 'static>(
    self_id: Id,
    other_ids: impl IntoIterator<Item = &'a Id>,
) -> impl Iterator<Item = &'a Id> {
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
        let ids: Vec<Id> = (0..3).map(Id::from).collect();
        assert_eq!(
            peer_ids(ids[1], &ids).collect::<Vec<&Id>>(),
            vec![&Id::from(0), &Id::from(2)]
        );
    }

    #[test]
    fn vec_can_serve_as_actor() {
        use crate::StateRecorder;
        use crate::{Checker, Expectation, Model};
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        ActorModel::new((), ())
            .actor(vec![(1.into(), 'A'), (1.into(), 'B')])
            .actor(vec![(0.into(), 'C'), (0.into(), 'D')])
            .property(Expectation::Always, "", |_, _| true)
            .checker()
            .visitor(recorder)
            .spawn_bfs()
            .join();
        let mut messages_by_state: Vec<_> = accessor()
            .into_iter()
            .map(|s| {
                let mut messages: Vec<_> = s.network.iter_deliverable().map(|e| *e.msg).collect();
                messages.sort();
                messages
            })
            .collect();
        messages_by_state.sort_by(|x, y| {
            // Sort by length, then by content.
            match x.len().cmp(&y.len()) {
                std::cmp::Ordering::Equal => x.cmp(y),
                order => order,
            }
        });
        assert_eq!(
            messages_by_state,
            [
                vec!['A', 'C'],
                vec!['A', 'B', 'C'],
                vec!['A', 'C', 'D'],
                vec!['A', 'B', 'C', 'D'],
                vec!['A', 'B', 'C', 'D'],
                vec!['A', 'B', 'C', 'D'],
                vec!['A', 'B', 'C', 'D'],
            ]
        );
    }
}
