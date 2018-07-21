//! A library for specifying state machines and model checking invariants.
//!
//! A small example follows. Please see the
//! [README](https://github.com/stateright/stateright/blob/master/README.md) for a more thorough
//! introduction.
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

extern crate difference;
extern crate fxhash;
extern crate regex;
extern crate serde;
#[cfg(test)]
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use fxhash::{FxHashMap, FxHashSet};
use regex::Regex;
use std::cmp::max;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::hash::Hash;

pub mod actor;

/// Represents an action-state pair.
pub type Step<State> = (&'static str, State);

/// Represents the range of action-state pairs that a state machine can follow during a step.
pub type StepVec<State> = Vec<Step<State>>;

/// Defines how a state begins and evolves, possibly nondeterministically.
pub trait StateMachine: Sized {
    /// The type of state upon which this machine operates.
    type State: Debug + Hash;

    /// Collects the initial possible action-state pairs.
    fn init(&self, results: &mut StepVec<Self::State>);

    /// Collects the subsequent possible action-state pairs based on a previous state.
    fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>);

    /// Initializes a fresh checker for a state machine.
    fn checker(&self, keep_paths: KeepPaths, invariant: fn(&Self, &Self::State) -> bool) -> Checker<Self> {
        const STARTING_CAPACITY: usize = 1_000_000;

        let mut results = StepVec::new();
        self.init(&mut results);
        let mut pending = VecDeque::new();
        for r in results { pending.push_back(r.1); }

        let mut source = FxHashMap::default();
        if keep_paths == KeepPaths::Yes {
            source = FxHashMap::with_capacity_and_hasher(STARTING_CAPACITY, Default::default());
            for init_state in &pending {
                let init_digest = hash(init_state);
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
    pending: VecDeque<SM::State>,
    source: FxHashMap<u64, Option<u64>>,
    pub visited: FxHashSet<u64>,
}

impl<'a, M: StateMachine> Checker<'a, M> {
    /// Visits up to a specified number of states checking the model's invariant. May return
    /// earlier when all states have been visited or a state is found in which the invariant fails
    /// to hold.
    pub fn check(&mut self, max_count: usize) -> CheckResult<M::State> {
        let mut remaining = max_count;

        while let Some(state) = self.pending.pop_front() {
            let digest = hash(&state);

            // skip if already visited
            if self.visited.contains(&digest) { continue; }
            self.visited.insert(digest);

            // collect the next steps/states
            let mut results = StepVec::new();
            self.state_machine.next(&state, &mut results);
            if self.keep_paths == KeepPaths::Yes {
                for r in &results {
                    let next_digest = hash(&r.1);
                    self.source.entry(next_digest).or_insert_with(|| Some(digest));
                }
            }
            for r in results { self.pending.push_back(r.1); }

            // exit if invariant fails to hold or we've reached the max count
            let inv = self.invariant;
            if !inv(&self.state_machine, &state) { return CheckResult::Fail { state }; }
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

        if self.keep_paths == KeepPaths::No { return None }

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
        Some(output)
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
                                 .unwrap_or_default());
                    if let Some(path) = self.path_to(&state) {
                        let newline_re = Regex::new(r"\n *").unwrap();
                        let control_re = Regex::new(r"\n *(?P<c>\x1B\[\d+m) *").unwrap();
                        let mut maybe_last_state_str: Option<String> = None;
                        for (action, state) in path {
                            // Pretty-print as that results in a more sane diff, then collapse the
                            // lines to keep the output more concise.
                            let state_str: String = format!("{:#?}", state);
                            print!("    \x1B[33m|{}|\x1B[0m ", action);
                            match maybe_last_state_str {
                                None => {
                                    println!("{}", newline_re.replace_all(&state_str, " "));
                                }
                                Some(last_str) => {
                                    let diff = format!("{}", difference::Changeset::new(&last_str, &state_str, "\n"));
                                    println!("{}",
                                             newline_re.replace_all(
                                                 &control_re.replace_all(&diff, "$c "),
                                                 " "));
                                }
                            }
                            maybe_last_state_str = Some(state_str);
                        }
                    }
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
    fn model_check_can_resume_after_failing() {
        let mut checker = LinearEquation { a: 0, b: 0, c: 0 }.checker(KeepPaths::No, invariant);
        // init case
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(0), Wrapping(0)) });
        // distance==1 cases
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(1), Wrapping(0)) });
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(0), Wrapping(1)) });
        // subset of distance==2 cases
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(2), Wrapping(0)) });
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(1), Wrapping(1)) });
        assert_eq!(checker.check(100), CheckResult::Fail { state: (Wrapping(0), Wrapping(2)) });
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

    #[test]
    fn model_check_can_omit_path() {
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(KeepPaths::No, invariant);
        match checker.check(100_000) {
            CheckResult::Fail { state } => {
                assert_eq!(checker.path_to(&state), None); // b/c not recorded
            },
            _ => panic!("expected solution")
        }
    }
}

