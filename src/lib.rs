//! A library for model checking systems, with an emphasis on distributed systems.
//!
//! Please see the
//! [examples](https://github.com/stateright/stateright/tree/master/examples),
//! [README](https://github.com/stateright/stateright/blob/master/README.md), and
//! submodules for additional details.
//!
//! The [`actor`] and [`semantics`] submodules will be of particular interest
//! to most individuals.

use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use crate::checker::Checker;

pub mod actor;
pub mod checker;
pub mod explorer;
pub mod semantics;
#[cfg(test)]
mod test_util;
pub mod util;

/// Models a possibly nondeterministic system's evolution. See [`Checker`].
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

    /// Summarizes the outcome of taking a step.
    fn display_outcome(&self, last_state: &Self::State, action: Self::Action) -> Option<String>
    where Self::State: Debug
    {
        self.next_state(last_state, action)
            .map(|next_state| format!("{:?}", next_state))
    }

    /// Indicates the steps (action-state pairs) that follow a particular state.
    fn next_steps(&self, last_state: &Self::State) -> Vec<(Self::Action, Self::State)>
    where Self::State: Hash
    {
        // Must generate the actions twice because they are consumed by `next_state`.
        let mut actions1 = Vec::new();
        let mut actions2 = Vec::new();
        self.actions(&last_state, &mut actions1);
        self.actions(&last_state, &mut actions2);
        actions1.into_iter().zip(actions2)
            .filter_map(|(action1, action2)|
                self.next_state(&last_state, action1).map(|state| (action2, state)))
            .collect()
    }

    /// Indicates the states that follow a particular state. Slightly more efficient than calling
    /// [`Model::next_steps`] and projecting out the states.
    fn next_states(&self, last_state: &Self::State) -> Vec<Self::State> {
        let mut actions = Vec::new();
        self.actions(&last_state, &mut actions);
        actions.into_iter()
            .filter_map(|action| self.next_state(&last_state, action))
            .collect()
    }

    /// Determines the final state associated with a particular fingerprint path.
    fn follow_fingerprints(&self, init_states: Vec<Self::State>, fingerprints: Vec<Fingerprint>) -> Option<Self::State>
    where Self::State: Hash
    {
        // Split the fingerprints into a head and tail. There are more efficient ways to do this,
        // but since this function is not performance sensitive, the implementation favors clarity.
        let mut remaining_fps = fingerprints;
        let expected_fp = remaining_fps.remove(0);

        for init_state in init_states {
            if fingerprint(&init_state) == expected_fp {
                let next_states = self.next_states(&init_state);
                return if remaining_fps.is_empty() {
                    Some(init_state)
                } else {
                    self.follow_fingerprints(next_states, remaining_fps)
                }
            }
        }
        None
    }

    /// Generates the expected properties for this model.
    fn properties(&self) -> Vec<Property<Self>> { Vec::new() }

    /// Indicates whether a state is within the state space that should be model checked.
    fn within_boundary(&self, _state: &Self::State) -> bool { true }

    /// Initializes a single threaded model checker.
    fn checker(self) -> Checker<Self> {
        self.checker_with_threads(1)
    }

    /// Initializes a model checker. The visitation order will be nondeterministic if
    /// `thread_count > 1`.
    fn checker_with_threads(self, thread_count: usize) -> Checker<Self> {
        use std::collections::VecDeque;
        use dashmap::DashMap;
        Checker {
            thread_count,
            model: self,
            pending: VecDeque::with_capacity(50_000),
            sources: DashMap::with_capacity_and_hasher(1_000_000, Default::default()),
            discoveries: DashMap::with_capacity(10),
        }
    }
}

/// A named predicate, such as "an epoch *sometimes* has no leader" (for which the the model
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
    pub fn always(name: &'static str, condition: fn(&M, &M::State) -> bool)
                  -> Property<M> {
        Property { expectation: Expectation::Always, name, condition }
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
    /// by the cycle-closing edge will ignored -- a false negative.
    pub fn eventually(name: &'static str, condition: fn(&M, &M::State) -> bool)
                      -> Property<M> {
        Property { expectation: Expectation::Eventually, name, condition }
    }

    /// Something that should be possible in the model. The model checker will try to discover an
    /// example.
    pub fn sometimes(name: &'static str, condition: fn(&M, &M::State) -> bool)
                     -> Property<M> {
        Property { expectation: Expectation::Sometimes, name, condition }
    }
}

/// Indicates whether a property is always, eventually, or sometimes true.
#[derive(Debug, Eq, PartialEq)]
pub enum Expectation {
    /// The property is true for all reachable states.
    Always,
    /// The property is eventually true for all behavior paths.
    Eventually,
    /// The property is true for at least one reachable state.
    Sometimes,
}

/// A state identifier. See [`fingerprint`].
pub type Fingerprint = std::num::NonZeroU64;

/// Converts a state to a [`Fingerprint`].
#[inline]
pub fn fingerprint<T: Hash>(value: &T) -> Fingerprint {
    let mut hasher = stable::hasher();
    value.hash(&mut hasher);
    Fingerprint::new(hasher.finish()).expect("hasher returned zero, an invalid fingerprint")
}

// Helpers for stable hashing, wherein hashes should not vary across builds.
mod stable {
    use ahash::{AHasher, RandomState};

    const KEY1: u64 = 123_456_789_987_654_321;
    const KEY2: u64 = 98_765_432_123_456_789;

    pub(crate) fn hasher() -> AHasher {
        AHasher::new_with_keys(KEY1, KEY2)
    }

    pub(crate) fn build_hasher() -> RandomState {
        RandomState::with_seeds(KEY1, KEY2)
    }
}
