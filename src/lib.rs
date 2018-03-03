//! A library for specifying state machines and model checking invariants.
//!
//! ## License
//!
//! Copyright 2018 Jonathan Nadal and made available under the MIT License.


use std::collections::VecDeque;
use std::hash::Hash;

/// Defines how a state begins and evolves, possibly nondeterministically.
pub trait StateMachine: Sized {
    /// The type of state upon which this machine operates.
    type State: Clone + Eq + Hash;

    /// Collects the initial possible description-state pairs.
    fn init(&self, results: &mut VecDeque<(&'static str, Self::State)>);

    /// Collects the subsequent possible description-state pairs based on a previous state.
    fn next(&self, state: &Self::State, results: &mut VecDeque<(&'static str, Self::State)>);
}

/// Elaborates on a state machine by providing a state invariant.
pub trait Model: StateMachine {
    /// A claim that should always be true.
    fn invariant(&self, state: &Self::State) -> bool;
}

