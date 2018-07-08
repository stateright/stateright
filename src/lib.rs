//! A library for specifying state machines and model checking invariants.
//!
//! ## Example
//!
//! As a simple example, we can simulate a minimal "clock" that alternates between
//! two hours: zero and one. Then we can enumerate all possible states verifying
//! that the time is always within bounds and that a path to the other hour begins
//! at the `start` hour (a model input) followed by a step for flipping the hour
//! bit.
//!
//! ```rust
//! use stateright::*;
//!
//! struct BinaryClock { start: u8 }
//!
//! impl StateMachine for BinaryClock {
//!     type State = u8;
//!
//!     fn init(&self, results: &mut StepVec<Self::State>) {
//!         results.push(("start", self.start));
//!     }
//!
//!     fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>) {
//!         results.push(("flip bit", (1 - *state)));
//!     }
//! }
//!
//! let mut checker = BinaryClock { start: 1 }.checker(
//!     KeepPaths::Yes,
//!     |clock, time| 0 <= *time && *time <= 1);
//! assert_eq!(
//!     checker.check(100),
//!     CheckResult::Pass);
//! assert_eq!(
//!     checker.path_to(&0),
//!     Some(vec![("start", 1), ("flip bit", 0)]));
//! ```
//!
//! ## More Examples
//!
//! See the [examples/state\_machines/](https://github.com/stateright/stateright/tree/master/examples/state_machines)
//! directory for additional examples, such as an actor based write-once register
//! and an abstract two phase commit state machine.
//!
//! ## Performance
//!
//! To benchmark model checking speed, run:
//!
//! ```sh
//! cargo run --release --example bench 2pc
//! cargo run --release --example bench wor
//! ```
//!
//! ## Contributing
//!
//! 1. Clone the repository:
//!    ```sh
//!    git clone https://github.com/stateright/stateright.git
//!    cd stateright
//!    ```
//! 2. Install the latest version of rust:
//!    ```sh
//!    rustup update || (curl https://sh.rustup.rs -sSf | sh)
//!    ```
//! 3. Run the tests:
//!    ```sh
//!    cargo test && cargo test --examples
//!    ```
//! 4. Review the docs:
//!    ```sh
//!    cargo doc --open
//!    ```
//! 5. Explore the code:
//!    ```sh
//!    $EDITOR src/ # src/lib.rs is a good place to start
//!    ```
//! 6. If you would like to share improvements, please
//!    [fork the library](https://github.com/stateright/stateright/fork), push changes to your fork,
//!    and send a [pull request](https://help.github.com/articles/creating-a-pull-request-from-a-fork/).
//!
//! ## License
//!
//! Copyright 2018 Jonathan Nadal and made available under the MIT License.

extern crate fxhash;

use fxhash::{FxHashMap, FxHashSet};
use std::cmp::max;
use std::collections::VecDeque;
use std::hash::Hash;

pub mod actor;

/// Represents an action-state pair.
pub type Step<State> = (&'static str, State);

/// Represents the range of action-state pairs that a state machine can follow during a step.
pub type StepVec<State> = Vec<Step<State>>;

/// Defines how a state begins and evolves, possibly nondeterministically.
pub trait StateMachine: Sized {
    /// The type of state upon which this machine operates.
    type State: Hash;

    /// Collects the initial possible action-state pairs.
    fn init(&self, results: &mut StepVec<Self::State>);

    /// Collects the subsequent possible action-state pairs based on a previous state.
    fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>);

    /// Initializes a fresh checker for a state machine.
    fn checker(&self, keep_paths: KeepPaths, invariant: fn(&Self, &Self::State) -> bool) -> Checker<Self> {
        const STARTING_CAPACITY: usize = 1_000_000;

        let mut results = StepVec::new();
        self.init(&mut results);
        let mut pending: VecDeque<Step<Self::State>> = VecDeque::new();
        for r in results { pending.push_back(r); }

        let mut source = FxHashMap::default();
        if keep_paths == KeepPaths::Yes {
            source = FxHashMap::with_capacity_and_hasher(STARTING_CAPACITY, Default::default());
            for &(ref _init_action, ref init_state) in pending.iter() {
                let init_digest = hash(&init_state);
                source.entry(init_digest).or_insert(None);
            }
        }

        Checker {
            invariant,
            keep_paths,
            state_machine: self,

            pending,
            source,
            visited: FxHashSet::with_capacity_and_hasher(STARTING_CAPACITY, Default::default()),
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

/// Use `KeepPaths::No` for faster model checking.
#[derive(PartialEq)]
pub enum KeepPaths { Yes, No }

/// Visits every state reachable by a state machine, and verifies that an invariant holds.
pub struct Checker<'a, SM: 'a + StateMachine> {
    // immutable cfg
    keep_paths: KeepPaths,
    state_machine: &'a SM,
    invariant: fn(&SM, &SM::State) -> bool,

    // mutable checking state
    pending: VecDeque<Step<SM::State>>,
    source: FxHashMap<u64, Option<u64>>,
    pub visited: FxHashSet<u64>,
}

impl<'a, M: StateMachine> Checker<'a, M> {
    /// Visits up to a specified number of states checking the model's invariant. May return
    /// earlier when all states have been visited or a state is found in which the invariant fails
    /// to hold.
    pub fn check(&mut self, max_count: usize) -> CheckResult<M::State> {
        let mut remaining = max_count;

        while let Some((_action, state)) = self.pending.pop_front() {
            let digest = hash(&state);

            // skip if already visited
            if self.visited.contains(&digest) { continue; }

            // exit if invariant fails to hold
            let inv = self.invariant;
            if !inv(&self.state_machine, &state) {
                self.visited.insert(digest);
                return CheckResult::Fail { state };
            }

            // otherwise collect the next steps/states
            let mut results = StepVec::new();
            self.state_machine.next(&state, &mut results);
            if self.keep_paths == KeepPaths::Yes {
                for &(ref _next_action, ref next_state) in &results {
                    let next_digest = hash(&next_state);
                    self.source.entry(next_digest).or_insert(Some(digest));
                }
            }
            for r in results { self.pending.push_back(r); }
            self.visited.insert(digest);

            // but pause if we've reached the limit so that the caller can display progress
            remaining -= 1;
            if remaining == 0 { return CheckResult::Incomplete }
        }

        CheckResult::Pass
    }

    /// Identifies the action-state "behavior" path by which a visited state was reached.
    pub fn path_to(&self, state: &M::State) -> Option<Vec<Step<M::State>>> {
        // First build a stack of digests representing the path (with the init digest at top of
        // stack). Then unwind the stack of digests into a vector of states. The TLC model checker
        // uses a similar technique, which is documented in the paper "Model Checking TLA+
        // Specifications" by Yu, Manolios, and Lamport.

        let find_step = |steps: StepVec<M::State>, digest: u64|
            steps.into_iter()
                .find(|step| hash(&step.1) == digest)
                .expect("step with state matching recorded digest");

        // 1. Build a stack of digests.
        let mut digests = Vec::new();
        let mut next_digest = hash(&state);
        while let Some(source) = self.source.get(&next_digest) {
            match *source {
                Some(prev_digest) => {
                    digests.push(next_digest);
                    next_digest = prev_digest;
                },
                None => {
                    digests.push(next_digest);
                    break;
                },
            }
        }

        // 2. Begin unwinding by determining the init step.
        let mut output = Vec::new();
        let mut steps = StepVec::new();
        self.state_machine.init(&mut steps);
        output.push(find_step(steps, digests.pop().expect("at least one state due to param")));

        // 3. Then continue with the remaining steps.
        while let Some(next_digest) = digests.pop() {
            let mut next_steps = StepVec::new();
            self.state_machine.next(
                &output.last().expect("nonempty (b/c step was already enqueued)").1,
                &mut next_steps);
            output.push(find_step(next_steps, next_digest));
        }
        return Some(output);
    }

    /// Blocks the thread until model checking is complete. Periodically emits a status while
    /// checking, tailoring the block size to the checking speed. Emits a report when complete.
    pub fn check_and_report(&mut self) {
        use std::time::Instant;
        let method_start = Instant::now();
        let mut block_size = 32_768;
        loop {
            let block_start = Instant::now();
            match self.check(block_size) {
                CheckResult::Fail { state } => {
                    println!("{} unique states visited after {} sec. Invariant violated{}.",
                             self.visited.len(),
                             method_start.elapsed().as_secs(),
                             self.path_to(&state)
                                 .map(|path| format!(" by path of length {}", path.len()))
                                 .unwrap_or(String::from("")));
                    return;
                },
                CheckResult::Pass => {
                    println!("{} unique states visited after {} sec. Passed.",
                             self.visited.len(),
                             method_start.elapsed().as_secs());
                    return;
                },
                CheckResult::Incomplete => {}
            }

            let block_elapsed = block_start.elapsed().as_secs();
            if block_elapsed > 0 {
                println!("{} unique states visited after {} sec. Continuing.",
                         self.visited.len(),
                         method_start.elapsed().as_secs());
            }

            if block_elapsed < 3 { block_size *= 2; }
            else if block_elapsed > 10 { block_size = max(1, block_size / 2); }
        }
    }
}

fn hash<T: Hash>(value: &T) -> u64 {
    fxhash::hash64(value)
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

        fn init(&self, results: &mut StepVec<Self::State>) {
            results.push(("guess", (Wrapping(0), Wrapping(0))));
        }

        fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>) {
            match *state {
                (x, y) => {
                    results.push(("increase x", (x + Wrapping(1), y)));
                    results.push(("increase y", (x, y + Wrapping(1))));
                }
            }
        }
    }
    fn invariant(equation: &LinearEquation, solution: &(Wrapping<u8>, Wrapping<u8>)) -> bool {
        match *solution {
            (x, y) => {
                Wrapping(equation.a)*x + Wrapping(equation.b)*y != Wrapping(equation.c)
            }
        }
    }

    #[test]
    fn model_check_records_states() {
        use std::iter::FromIterator;
        let h = |a: u8, b: u8| hash(&(Wrapping(a), Wrapping(b)));
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(KeepPaths::No, invariant);
        checker.check(100);
        assert_eq!(checker.visited, FxHashSet::from_iter(vec![
            h(0, 0),
            h(1, 0), h(0, 1),
            h(2, 0), h(1, 1), h(0, 2),
            h(3, 0), h(2, 1),
        ]));
    }

    #[test]
    fn model_check_can_pass() {
        let mut checker = LinearEquation { a: 2, b: 4, c: 7 }.checker(KeepPaths::No, invariant);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.visited.len(), 100);
        assert_eq!(checker.check(100_000), CheckResult::Pass);
        assert_eq!(checker.visited.len(), 256 * 256);
    }

    #[test]
    fn model_check_can_fail() {
        let mut checker = LinearEquation { a: 2, b: 7, c: 111 }.checker(KeepPaths::No, invariant);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.visited.len(), 100);
        assert_eq!(
            checker.check(100_000),
            CheckResult::Fail { state: (Wrapping(3), Wrapping(15)) });
        assert_eq!(checker.visited.len(), 187);
    }

    #[test]
    fn model_check_can_indicate_path() {
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(KeepPaths::Yes, invariant);
        match checker.check(100_000) {
            CheckResult::Fail { state } => {
                assert_eq!(
                    checker.path_to(&state),
                    Some(vec![
                        ("guess",      (Wrapping(0), Wrapping(0))),
                        ("increase x", (Wrapping(1), Wrapping(0))),
                        ("increase x", (Wrapping(2), Wrapping(0))),
                        ("increase y", (Wrapping(2), Wrapping(1))),
                    ]));
            },
            _ => panic!("expected solution")
        }
    }
}

