//! A library for model checking systems, with an emphasis on distributed systems.
//!
//! Most of the docs are here, but a short book is being written to provide a gentler introduction:
//! "[Building Distributed Systems With Stateright](https://www.stateright.rs)."
//!
//! # Introduction to Model Checking
//!
//! [`Model`] implementations indicate how a system evolves, such as a set of actors
//! executing a distributed protocol on an IP network. Incidentally, that scenario
//! is so common for model checking that Stateright includes an [`actor::ActorModel`],
//! and unlike many model checkers, Stateright is also able to [spawn] these
//! actors on a real network.
//!
//! Models of a system are supplemented with [`always`] and [`sometimes`] properties.
//! An `always` [`Property`] (also known as a [safety property] or [invariant]) indicates that a
//! specific problematic outcome is not possible, such as data inconsistency. A
//! `sometimes` property on the other hand is used to ensure a particular outcome
//! is reachable, such as the ability for a distributed system to process a write
//! request.
//!
//! A [`Checker`] will attempt to [discover] a counterexample
//! for every `always` property and an example for every `sometimes` property,
//! and these examples/counterexamples are indicated by sequences of system steps
//! known as [`Path`]s (also known as traces or behaviors). The presence of an
//! `always` discovery or the absence of a `sometimes` discovery indicate that
//! the model checker identified a problem with the code under test.
//!
//! # Example
//!
//! A toy example that solves a [sliding puzzle](https://en.wikipedia.org/wiki/Sliding_puzzle)
//! follows. This simple example leverages only a `sometimes` property,
//! but in most cases for a real system you would have an `always` property at
//! a minimum. The imagined use case is one in which we must ensure that a particular
//! configuration of the puzzle has a solution.
//!
//! **TIP**: see the [`actor`] module documentation for an actor system example.
//! More sophisticated examples
//! are available in the [`examples/` directory of the repository](https://github.com/stateright/stateright/tree/master/examples)
//!
//! ```rust
//! use stateright::*;
//!
//! #[derive(Clone, Debug, Eq, PartialEq)]
//! enum Slide { Down, Up, Right, Left }
//!
//! struct Puzzle([u8; 9]);
//! impl Model for Puzzle {
//!     type State = [u8; 9];
//!     type Action = Slide;
//!
//!     fn init_states(&self) -> Vec<Self::State> {
//!         vec![self.0]
//!     }
//!
//!     fn actions(&self, _state: &Self::State, actions: &mut Vec<Self::Action>) {
//!         actions.append(&mut vec![
//!             Slide::Down, Slide::Up, Slide::Right, Slide::Left
//!         ]);
//!     }
//!
//!     fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
//!         let empty = last_state.iter().position(|x| *x == 0).unwrap();
//!         let empty_y = empty / 3;
//!         let empty_x = empty % 3;
//!         let maybe_from = match action {
//!             Slide::Down  if empty_y > 0 => Some(empty - 3), // above
//!             Slide::Up    if empty_y < 2 => Some(empty + 3), // below
//!             Slide::Right if empty_x > 0 => Some(empty - 1), // left
//!             Slide::Left  if empty_x < 2 => Some(empty + 1), // right
//!             _ => None
//!         };
//!         maybe_from.map(|from| {
//!             let mut next_state = *last_state;
//!             next_state[empty] = last_state[from];
//!             next_state[from] = 0;
//!             next_state
//!         })
//!     }
//!
//!     fn properties(&self) -> Vec<Property<Self>> {
//!         vec![Property::<Self>::sometimes("solved", |_, state| {
//!             let solved = [0, 1, 2,
//!                           3, 4, 5,
//!                           6, 7, 8];
//!             state == &solved
//!         })]
//!     }
//! }
//! let checker = Puzzle([1, 4, 2,
//!                       3, 5, 8,
//!                       6, 7, 0])
//!     .checker().spawn_bfs().join();
//! checker.assert_properties();
//! checker.assert_discovery("solved", vec![
//!         Slide::Down,
//!         // ... results in:
//!         //       [1, 4, 2,
//!         //        3, 5, 0,
//!         //        6, 7, 8]
//!         Slide::Right,
//!         // ... results in:
//!         //       [1, 4, 2,
//!         //        3, 0, 5,
//!         //        6, 7, 8]
//!         Slide::Down,
//!         // ... results in:
//!         //       [1, 0, 2,
//!         //        3, 4, 5,
//!         //        6, 7, 8]
//!         Slide::Right,
//!         // ... results in:
//!         //       [0, 1, 2,
//!         //        3, 4, 5,
//!         //        6, 7, 8]
//!     ]);
//! ```
//!
//! # What to Read Next
//!
//! The [`actor`] and [`semantics`] submodules will be of particular interest to
//! most individuals.
//!
//! Also, as mentioned earlier, you can find [more examples](https://github.com/stateright/stateright/tree/master/examples)
//! in the Stateright repository.
//!
//! [`always`]: Property::always
//! [discover]: Checker::discoveries
//! [invariant]: https://en.wikipedia.org/wiki/Invariant_(computer_science)
//! [`Model`]: Model
//! [safety property]: https://en.wikipedia.org/wiki/Safety_property
//! [`sometimes`]: Property::sometimes
//! [spawn]: actor::spawn()

#[warn(anonymous_parameters)]
#[warn(missing_docs)]
mod checker;
mod has_discoveries;
mod job_market;
pub mod report;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

#[cfg(test)]
mod test_util;

pub mod actor;
pub use checker::*;
pub use has_discoveries::HasDiscoveries;
pub mod semantics;
pub mod util;

/// This is the primary abstraction for Stateright. Implementations model a
/// nondeterministic system's evolution. If you are using Stateright's actor framework,
/// then you do not need to implement this interface and can instead leverage
/// [`actor::Actor`] and [`actor::ActorModel`].
///
/// You can instantiate a model [`CheckerBuilder`] by calling [`Model::checker`].
pub trait Model: Sized {
    /// The type of state upon which this model operates.
    type State;

    /// The type of action that transitions between states.
    type Action;

    /// Returns the initial possible states.
    fn init_states(&self) -> Vec<Self::State>;

    /// Collects the subsequent possible actions based on a previous state.
    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>);

    /// Converts a previous state and action to a resulting state. [`None`] indicates that the action
    /// does not change the state.
    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State>;

    /// Converts an action of this model to a more intuitive representation (e.g. for Explorer).
    fn format_action(&self, action: &Self::Action) -> String
    where
        Self::Action: Debug,
    {
        format!("{:?}", action)
    }

    /// Converts a step of this model to a more intuitive representation (e.g. for Explorer).
    fn format_step(&self, last_state: &Self::State, action: Self::Action) -> Option<String>
    where
        Self::State: Debug,
    {
        self.next_state(last_state, action)
            .map(|next_state| format!("{:#?}", next_state))
    }

    /// Returns an [SVG](https://developer.mozilla.org/en-US/docs/Web/SVG) representation of a
    /// [`Path`] for this model.
    fn as_svg(&self, _path: Path<Self::State, Self::Action>) -> Option<String> {
        None
    }

    /// Indicates the steps (action-state pairs) that follow a particular state.
    fn next_steps(&self, last_state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        // Must generate the actions twice because they are consumed by `next_state`.
        let mut actions1 = Vec::new();
        let mut actions2 = Vec::new();
        self.actions(last_state, &mut actions1);
        self.actions(last_state, &mut actions2);
        actions1
            .into_iter()
            .zip(actions2)
            .filter_map(|(action1, action2)| {
                self.next_state(last_state, action1)
                    .map(|state| (action2, state))
            })
            .collect()
    }

    /// Indicates the states that follow a particular state. Slightly more efficient than calling
    /// [`Model::next_steps`] and projecting out the states.
    fn next_states(&self, last_state: &Self::State) -> Vec<Self::State> {
        let mut actions = Vec::new();
        self.actions(last_state, &mut actions);
        actions
            .into_iter()
            .filter_map(|action| self.next_state(last_state, action))
            .collect()
    }

    /// Generates the expected properties for this model.
    fn properties(&self) -> Vec<Property<Self>> {
        Vec::new()
    }

    /// Looks up a property by name. Panics if the property does not exist.
    fn property(&self, name: &'static str) -> Property<Self> {
        if let Some(p) = self.properties().into_iter().find(|p| p.name == name) {
            p
        } else {
            let available: Vec<_> = self.properties().iter().map(|p| p.name).collect();
            panic!(
                "Unknown property. requested={}, available={:?}",
                name, available
            );
        }
    }

    /// Indicates whether a state is within the state space that should be model checked.
    fn within_boundary(&self, _state: &Self::State) -> bool {
        true
    }

    /// Instantiates a [`CheckerBuilder`] for this model.
    fn checker(self) -> CheckerBuilder<Self>
    where
        Self: Send + Sync + 'static,
        Self::State: Hash + Send + Sync,
    {
        CheckerBuilder::new(self)
    }
}

/// A named predicate, such as "an epoch *sometimes* has no leader" (for which the model
/// checker would find an example) or "an epoch *always* has at most one leader" (for which the
/// model checker would find a counterexample) or "a proposal is *eventually* accepted" (for
/// which the model checker would find a counterexample path leading from the initial state
/// through to a terminal state).
pub struct Property<M: Model> {
    pub expectation: Expectation,
    pub name: &'static str,
    pub condition: fn(&M, &M::State) -> bool,
}
impl<M: Model> Property<M> {
    /// An invariant that defines a [safety
    /// property](https://en.wikipedia.org/wiki/Safety_property). The model checker will try to
    /// discover a counterexample.
    pub fn always(name: &'static str, condition: fn(&M, &M::State) -> bool) -> Property<M> {
        Property {
            expectation: Expectation::Always,
            name,
            condition,
        }
    }

    /// An invariant that defines a [liveness
    /// property](https://en.wikipedia.org/wiki/Liveness). The model checker will try to
    /// discover a counterexample path leading from the initial state through to a
    /// terminal state.
    ///
    /// Note that in this implementation `eventually` properties only work correctly on acyclic
    /// paths (those that end in either states with no successors or checking boundaries). A path
    /// ending in a cycle is not viewed as _terminating_ in that cycle, as the checker does not
    /// differentiate cycles from DAG joins, and so an `eventually` property that has not been met
    /// by the cycle-closing edge will be ignored -- a false negative.
    pub fn eventually(name: &'static str, condition: fn(&M, &M::State) -> bool) -> Property<M> {
        Property {
            expectation: Expectation::Eventually,
            name,
            condition,
        }
    }

    /// Something that should be possible in the model. The model checker will try to discover an
    /// example.
    pub fn sometimes(name: &'static str, condition: fn(&M, &M::State) -> bool) -> Property<M> {
        Property {
            expectation: Expectation::Sometimes,
            name,
            condition,
        }
    }
}
impl<M: Model> Clone for Property<M> {
    fn clone(&self) -> Self {
        Property {
            expectation: self.expectation.clone(),
            name: self.name,
            condition: self.condition,
        }
    }
}

/// Indicates whether a property is always, eventually, or sometimes true.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize)]
pub enum Expectation {
    /// The property is true for all reachable states.
    Always,
    /// The property is eventually true for all behavior paths.
    Eventually,
    /// The property is true for at least one reachable state.
    Sometimes,
}

impl Expectation {
    pub const fn discovery_is_failure(&self) -> bool {
        match self {
            Expectation::Always => true,
            Expectation::Eventually => true,
            Expectation::Sometimes => false,
        }
    }
}

/// A state identifier. See [`fingerprint`].
type Fingerprint = std::num::NonZeroU64;

/// Converts a state to a [`Fingerprint`].
#[inline]
fn fingerprint<T: Hash>(value: &T) -> Fingerprint {
    let mut hasher = stable::hasher();
    value.hash(&mut hasher);
    Fingerprint::new(hasher.finish()).expect("hasher returned zero, an invalid fingerprint")
}

/// Implemented only for rustdoc. Do not take a dependency on this. It will likely be removed in a
/// future version of this library.
#[doc(hidden)]
impl Model for () {
    type State = ();
    type Action = ();
    fn init_states(&self) -> Vec<Self::State> {
        vec![()]
    }
    fn actions(&self, _: &Self::State, actions: &mut Vec<Self::Action>) {
        actions.push(())
    }
    fn next_state(&self, _: &Self::State, _: Self::Action) -> Option<Self::State> {
        Some(())
    }
}

// Helpers for stable hashing, wherein hashes should not vary across builds.
mod stable {
    use std::hash::BuildHasher;

    use ahash::{AHasher, RandomState};

    const KEY1: u64 = 123_456_789_987_654_321;
    const KEY2: u64 = 98_765_432_123_456_789;
    // TODO: how to get these?
    const KEY3: u64 = 0;
    const KEY4: u64 = 0;

    pub(crate) fn hasher() -> AHasher {
        build_hasher().build_hasher()
    }

    pub(crate) fn build_hasher() -> RandomState {
        RandomState::with_seeds(KEY1, KEY2, KEY3, KEY4)
    }
}
