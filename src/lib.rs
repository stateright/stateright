//! A library for implementing state machines, in particular those defining distributed systems.
//!
//! Please see the
//! [examples](https://github.com/stateright/stateright/tree/master/examples),
//! [README](https://github.com/stateright/stateright/blob/master/README.md), and
//! submodules for additional details.

use std::fmt::Debug;
use std::hash::Hash;

pub mod actor;
pub mod checker;
pub mod explorer;
#[cfg(test)]
pub mod test_util;

/// Defines how a state begins and evolves, possibly nondeterministically.
pub trait StateMachine: Sized {
    /// The type of state upon which this machine operates.
    type State;

    /// The type of action that transitions between states.
    type Action;

    /// Returns the initial possible states.
    fn init_states(&self) -> Vec<Self::State>;

    /// Collects the subsequent possible actions based on a previous state.
    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>);

    /// Converts a previous state and action to a resulting state. `None` indicates that the action
    /// does not change the state.
    fn next_state(&self, last_state: &Self::State, action: &Self::Action) -> Option<Self::State>;

    /// Summarizes the outcome of taking a step.
    fn display_outcome(&self, last_state: &Self::State, action: &Self::Action) -> Option<String>
    where Self::State: Debug
    {
        self.next_state(last_state, action)
            .map(|next_state| format!("{:?}", next_state))
    }

    /// Indicates the steps (action-state pairs) that follow a particular state.
    fn next_steps(&self, last_state: &Self::State) -> Vec<(Self::Action, Self::State)>
    where Self::State: Hash
    {
        let mut actions = Vec::new();
        self.actions(&last_state, &mut actions);
        actions.into_iter()
            .filter_map(|action| {
                // Not every action results in a state, so we filter the actions by those that
                // generate a state. We also attach the action.
                self.next_state(&last_state, &action)
                    .map(|next_state| (action, next_state))
            })
            .collect()
    }

    /// Indicates the states that follow a particular state. Slightly more efficient than calling
    /// `next_steps` and projecting out the states.
    fn next_states(&self, last_state: &Self::State) -> Vec<Self::State> {
        let mut actions = Vec::new();
        self.actions(&last_state, &mut actions);
        actions.into_iter()
            .filter_map(|action| self.next_state(&last_state, &action))
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
}

/// A convenience structure for succinctly describing a throwaway `StateMachine`.
#[derive(Clone)]
pub struct QuickMachine<State, Action> {
    /// Returns the initial possible states.
    pub init_states: fn() -> Vec<State>,

    /// Collects the subsequent possible actions based on a previous state.
    pub actions: fn(&State, &mut Vec<Action>),

    /// Converts a previous state and action to a resulting state. `None` indicates that the action
    /// does not change the state.
    pub next_state: fn(&State, &Action) -> Option<State>,
}

impl<State, Action> StateMachine for QuickMachine<State, Action> {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<Self::State> {
        (self.init_states)()
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        (self.actions)(state, actions);
    }

    fn next_state(&self, last_state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        (self.next_state)(last_state, action)
    }
}

/// A state identifier. See `fingerprint`.
pub type Fingerprint = u64;

/// Converts a state to a fingerprint.
pub fn fingerprint<T: Hash>(value: &T) -> Fingerprint {
    fxhash::hash64(value)
}
