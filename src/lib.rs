//! A library for specifying state machines and model checking invariants.
//!
//! ## License
//!
//! Copyright 2018 Jonathan Nadal and made available under the MIT License.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;

/// Defines how a state begins and evolves, possibly nondeterministically.
pub trait StateMachine: Sized {
    /// The type of state upon which this machine operates.
    type State: Clone + Eq + Hash;

    /// Collects the initial possible action-state pairs.
    fn init(&self, results: &mut VecDeque<(&'static str, Self::State)>);

    /// Collects the subsequent possible action-state pairs based on a previous state.
    fn next(&self, state: &Self::State, results: &mut VecDeque<(&'static str, Self::State)>);
}

/// Elaborates on a state machine by providing a state invariant.
pub trait Model: StateMachine {
    /// A claim that should always be true.
    fn invariant(&self, state: &Self::State) -> bool;

    /// Initializes a fresh checker for a particular model.
    fn checker(&self, keep_paths: bool) -> Checker<Self> {
        const STARTING_CAPACITY: usize = 30_000_000;

        let mut pending: VecDeque<(&'static str, Self::State)> = VecDeque::new();
        self.init(&mut pending);

        let mut source: HashMap<Self::State, (&'static str, Option<Self::State>)> = HashMap::new();
        if keep_paths {
            source = HashMap::with_capacity(STARTING_CAPACITY);
            for &(ref action, ref next_state) in pending.iter() {
                if !source.contains_key(&next_state) {
                    source.insert(next_state.clone(), (action, None));
                }
            }
        }

        Checker {
            keep_paths,
            model: self,

            pending,
            source,
            visited: HashSet::with_capacity(STARTING_CAPACITY),
        }
    }
}

/// Model checking can be time consuming, so the library checks up to a fixed number of states then
/// returns. This approach allows the library to avoid tying up a thread indefinitely while still
/// maintaining adequate performance. This type represents the result of one of those checking
/// passes.
#[derive(Debug, Eq, PartialEq)]
pub enum CheckResult<State> {
    /// Indicates that the checker still has pending states.
    Incomplete,
    /// Indicates that checking completed, and the invariant was not violated.
    Pass,
    /// Indicates that checking completed, and the invariant did not hold.
    Fail { state: State }
}

/// Visits every state reachable by a state machine, and verifies that an invariant holds.
pub struct Checker<'model, M: 'model + Model> {
    // immutable cfg
    keep_paths: bool,
    model: &'model M,

    // mutable checking state
    pending: VecDeque<(&'static str, M::State)>,
    source: HashMap<M::State, (&'static str, Option<M::State>)>,
    pub visited: HashSet<M::State>,
}

impl<'model, M: Model> Checker<'model, M> {
    /// Visits up to a specified number of states checking the model's invariant. May return
    /// earlier when all states have been visited or a state is found in which the invariant fails
    /// to hold.
    pub fn check(&mut self, max_count: usize) -> CheckResult<M::State> {
        let mut remaining = max_count;

        while let Some((_action, state)) = self.pending.pop_front() {
            // skip if already visited
            if self.visited.contains(&state) { continue; }

            // exit if invariant fails to hold
            if !self.model.invariant(&state) {
                self.visited.insert(state.clone());
                return CheckResult::Fail { state };
            }

            // otherwise collect the next steps/states
            let before = self.pending.len();
            self.model.next(&state, &mut self.pending);
            if self.keep_paths {
                for &(ref next_action, ref next_state) in self.pending.iter().skip(before) {
                    if !self.source.contains_key(&next_state) {
                        self.source.insert(next_state.clone(), (next_action, Some(state.clone())));
                    }
                }
            }
            self.visited.insert(state);

            // but pause if we've reached the limit so that the caller can display progress
            remaining -= 1;
            if remaining == 0 { return CheckResult::Incomplete }
        }

        CheckResult::Pass
    }
}

#[cfg(test)]
mod test {
    use ::*;
    use std::num::Wrapping;

    /// Given `a`, `b`, and `c`, finds `x` and `y` such that `a*x + b*y = c` where all values are
    /// in `Wrapping<u8>`.
    struct LinearEquation { a: u8, b: u8, c: u8 }
    impl StateMachine for LinearEquation {
        type State = (Wrapping<u8>, Wrapping<u8>);

        fn init(&self, new_states: &mut VecDeque<(&'static str, Self::State)>) {
            new_states.push_back(("guess", (Wrapping(0), Wrapping(0))));
        }

        fn next(&self, state: &Self::State, new_states: &mut VecDeque<(&'static str, Self::State)>) {
            match *state {
                (x, y) => {
                    new_states.push_back(("increase x", (x + Wrapping(1), y)));
                    new_states.push_back(("increase y", (x, y + Wrapping(1))));
                }
            }
        }
    }
    impl Model for LinearEquation {
        fn invariant(&self, state: &Self::State) -> bool {
            match *state {
                (x, y) => {
                    Wrapping(self.a)*x + Wrapping(self.b)*y != Wrapping(self.c)
                }
            }
        }
    }

    #[test]
    fn model_check_records_states() {
        use std::iter::FromIterator;
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(false);
        checker.check(100);
        assert_eq!(checker.visited, HashSet::from_iter(vec![
            (Wrapping(0), Wrapping(0)),
            (Wrapping(1), Wrapping(0)), (Wrapping(0), Wrapping(1)),
            (Wrapping(2), Wrapping(0)), (Wrapping(1), Wrapping(1)), (Wrapping(0), Wrapping(2)),
            (Wrapping(3), Wrapping(0)), (Wrapping(2), Wrapping(1))]));
    }

    #[test]
    fn model_check_can_pass() {
        let mut checker = LinearEquation { a: 2, b: 4, c: 7 }.checker(false);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.visited.len(), 100);
        assert_eq!(checker.check(100_000), CheckResult::Pass);
        assert_eq!(checker.visited.len(), 256 * 256);
    }

    #[test]
    fn model_check_can_fail() {
        let mut checker = LinearEquation { a: 2, b: 7, c: 111 }.checker(false);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.visited.len(), 100);
        assert_eq!(
            checker.check(100_000),
            CheckResult::Fail { state: (Wrapping(3), Wrapping(15)) });
        assert_eq!(checker.visited.len(), 187);
    }
}

