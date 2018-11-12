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
//!     |clock, time| 0 <= *time && *time <= 1);
//! assert_eq!(
//!     checker.check(100),
//!     CheckResult::Pass);
//! assert_eq!(
//!     checker.path_to(&0),
//!     vec![("start", 1), ("flip bit", 0)]);
//! ```

extern crate difference;
extern crate fxhash;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use fxhash::FxHashMap;
use regex::Regex;
use std::cmp::max;
use std::collections::hash_map::Entry;
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
    type State;

    /// Collects the initial possible action-state pairs.
    fn init(&self, results: &mut StepVec<Self::State>);

    /// Collects the subsequent possible action-state pairs based on a previous state.
    fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>);

    /// Initializes a fresh checker for a state machine.
    fn checker<I>(&self, invariant: I) -> Checker<Self, I>
    where
        Self::State: Hash,
        I: Fn(&Self, &Self::State) -> bool,
    {
        Checker { workers: vec![Worker::init(self, invariant)] }
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

/// Generates every state reachable by a state machine, and verifies that an invariant holds.
pub struct Checker<'a, SM, I>
where
    SM: 'a + StateMachine,
    I: Fn(&SM, &SM::State) -> bool,
{
    workers: Vec<Worker<'a, SM, I>>,
}

impl<'a, SM, I> Checker<'a, SM, I>
where
    SM: 'a + StateMachine,
    SM::State: Hash,
    I: Fn(&SM, &SM::State) -> bool,
{
    /// Visits up to a specified number of states checking the model's invariant. May return
    /// earlier when all states have been checked or a state is found in which the invariant fails
    /// to hold. If the checker is using multiple workers, then each will visit the specified
    /// number of states.
    pub fn check(&mut self, max_count: usize) -> CheckResult<SM::State>
    where I: Send, SM: Sync, SM::State: Debug + Send
    {
        crossbeam_utils::thread::scope(|scope| {
            // 1. Kick off every worker.
            let mut threads = Vec::new();
            for worker in self.workers.iter_mut() {
                threads.push(scope.spawn(move |_| worker.check(max_count)));
            }

            // 2. Join.
            let mut results = Vec::new();
            for thread in threads {
                results.push(thread.join().unwrap());
            }

            // 3. Consolidate results.
            let all_passed = results.iter().all(|r| {
                match r {
                    CheckResult::Pass => true,
                    _ => false,
                }
            });
            if all_passed { return CheckResult::Pass }
            for result in results {
                if let CheckResult::Fail { state } = result {
                    return CheckResult::Fail { state };
                }
            }
            CheckResult::Incomplete
        }).unwrap()
    }

    /// Identifies the action-state "behavior" path by which a generated state was reached.
    pub fn path_to(&self, state: &SM::State) -> Vec<Step<SM::State>> {
        // First build a stack of digests representing the path (with the init digest at top of
        // stack). Then unwind the stack of digests into a vector of states. The TLC model checker
        // uses a similar technique, which is documented in the paper "Model Checking TLA+
        // Specifications" by Yu, Manolios, and Lamport.

        let find_step = |steps: StepVec<SM::State>, digest: u64|
            steps.into_iter()
                .find(|step| hash(&step.1) == digest)
                .expect("step with state matching recorded digest");
        let state_machine = self.workers.first().unwrap().state_machine;
        let sources = self.sources();

        // 1. Build a stack of digests.
        let mut digests = Vec::new();
        let mut next_digest = hash(&state);
        while let Some(source) = sources.get(&next_digest) {
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
        state_machine.init(&mut steps);
        output.push(find_step(steps, digests.pop().expect("at least one state due to param")));

        // 3. Then continue with the remaining steps.
        while let Some(next_digest) = digests.pop() {
            let mut next_steps = StepVec::new();
            state_machine.next(
                &output.last().expect("nonempty (b/c step was already enqueued)").1,
                &mut next_steps);
            output.push(find_step(next_steps, next_digest));
        }
        output
    }

    /// Blocks the thread until model checking is complete. Periodically emits a status while
    /// checking, tailoring the block size to the checking speed. Emits a report when complete.
    pub fn check_and_report(&mut self)
    where I: Copy + Send, SM: Sync, SM::State: Debug + Send
    {
        use std::time::Instant;
        let num_cpus = num_cpus::get();
        let method_start = Instant::now();
        let mut block_size = 32_768;
        loop {
            let block_start = Instant::now();
            match self.check(block_size) {
                CheckResult::Fail { state } => {
                    // First a quick summary.
                    let path = self.path_to(&state);
                    println!("{} states pending after {} sec. Invariant violated by path of length {}.",
                             self.pending_count(),
                             method_start.elapsed().as_secs(),
                             path.len());

                    // Then show the path.
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
                    return;
                },
                CheckResult::Pass => {
                    println!("Passed after {} sec.",
                             method_start.elapsed().as_secs());
                    return;
                },
                CheckResult::Incomplete => {}
            }

            let block_elapsed = block_start.elapsed().as_secs();
            if block_elapsed > 0 {
                println!("{} states pending after {} sec. Continuing.",
                         self.pending_count(),
                         method_start.elapsed().as_secs());
            }

            // Shrink or grow block if necessary. Otherwise adjust workers based on block size.
            if block_elapsed < 2 { block_size = 3 * block_size / 2; }
            else if block_elapsed > 10 { block_size = max(1, block_size / 2); }
            else {
                let threshold = max(1, block_size / num_cpus / 2);
                let queues: Vec<_> = self.workers.iter()
                    .map(|w| w.pending.len()).collect();
                println!("  cores={} threshold={} queues={:?}",
                         num_cpus, threshold, queues);
                self.adjust_worker_count(num_cpus, threshold);
            }
        }
    }

    /// By default a checker has one worker. This method forks workers whose pending queue size
    /// exceeds a specified threshold (while staying below a target worker count).
    pub fn adjust_worker_count(&mut self, target: usize, min_pending: usize)
    where I: Copy
    {
        let mut added = Vec::new();
        loop {
            let existing_count = self.workers.iter()
                .filter(|w| !w.pending.is_empty()).count();
            for worker in &mut self.workers {
                if existing_count + added.len() >= target { break }
                if worker.pending.len() < min_pending { continue }
                added.push(worker.fork());
            }

            if added.is_empty() { return }
            self.workers.append(&mut added);
        }
    }

    /// Indicates how many states are pending. If extra workers were created, this number may
    /// include duplicates.
    pub fn pending_count(&self) -> usize {
        self.workers.iter().map(|w| w.pending.len()).sum()
    }

    /// Indicates state sources by digest.
    pub fn sources(&self) -> FxHashMap<u64, Option<u64>> {
        let max_capacity = self.workers.iter().map(|w| w.sources.capacity()).max().unwrap();
        let mut sources = FxHashMap::with_capacity_and_hasher(2 * max_capacity, Default::default());
        for worker in &self.workers { sources.extend(worker.sources.clone()); }
        sources
    }
}

struct Worker<'a, SM, I>
where
    SM: 'a + StateMachine,
    I: Fn(&SM, &SM::State) -> bool,
{
    // immutable cfg
    invariant: I,
    state_machine: &'a SM,

    // mutable checking state
    pending: VecDeque<SM::State>,
    sources: FxHashMap<u64, Option<u64>>,
}

impl<'a, SM, I> Worker<'a, SM, I>
where
    SM: 'a + StateMachine,
    SM::State: Hash,
    I: Fn(&SM, &SM::State) -> bool,
{
    fn init(state_machine: &'a SM, invariant: I) -> Worker<'a, SM, I> {
        const STARTING_CAPACITY: usize = 1_000_000;

        let mut results = StepVec::new();
        state_machine.init(&mut results);
        let mut pending = VecDeque::new();
        let mut sources = FxHashMap::with_capacity_and_hasher(STARTING_CAPACITY, Default::default());
        for (_, init_state) in results {
            let init_digest = hash(&init_state);
            if let Entry::Vacant(init_source) = sources.entry(init_digest) {
                init_source.insert(None);
                pending.push_back(init_state);
            }
        }

        Worker {
            invariant,
            state_machine,

            pending,
            sources,
        }
    }

    fn fork(&mut self) -> Worker<'a, SM, I>
    where I: Copy
    {
        let len = self.pending.len() / 2;
        Worker {
            invariant: self.invariant,
            state_machine: self.state_machine,

            pending: self.pending.split_off(len),
            sources: self.sources.clone(),
        }
    }

    fn check(&mut self, max_count: usize) -> CheckResult<SM::State> {
        let mut remaining = max_count;

        while let Some(state) = self.pending.pop_front() {
            let digest = hash(&state);

            // collect the next steps/states
            let mut results = StepVec::new();
            self.state_machine.next(&state, &mut results);
            for (_, next_state) in results {
                let next_digest = hash(&next_state);
                if let Entry::Vacant(next_entry) = self.sources.entry(next_digest) {
                    next_entry.insert(Some(digest));
                    self.pending.push_back(next_state);
                }
            }

            // exit if invariant fails to hold or we've reached the max count
            let inv = &self.invariant;
            if !inv(&self.state_machine, &state) { return CheckResult::Fail { state }; }
            remaining -= 1;
            if remaining == 0 { return CheckResult::Incomplete }
        }

        CheckResult::Pass
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
        use fxhash::FxHashSet;
        use std::iter::FromIterator;

        let h = |a: u8, b: u8| hash(&(Wrapping(a), Wrapping(b)));
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(invariant);
        checker.check(100);
        let state_space = FxHashSet::from_iter(checker.sources().keys().cloned());
        assert!(state_space.contains(&h(0, 0)));
        assert!(state_space.contains(&h(1, 0)));
        assert!(state_space.contains(&h(0, 1)));
        assert!(state_space.contains(&h(2, 0)));
        assert!(state_space.contains(&h(1, 1)));
        assert!(state_space.contains(&h(0, 2)));
        assert!(state_space.contains(&h(3, 0)));
        assert!(state_space.contains(&h(2, 1)));
        assert_eq!(state_space.len(), 13); // not all generated were checked
    }

    #[test]
    fn model_check_can_pass() {
        let mut checker = LinearEquation { a: 2, b: 4, c: 7 }.checker(invariant);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.sources().len(), 115); // not all generated were checked
        assert_eq!(checker.check(100_000), CheckResult::Pass);
        assert_eq!(checker.sources().len(), 256 * 256);
    }

    #[test]
    fn model_check_can_fail() {
        let mut checker = LinearEquation { a: 2, b: 7, c: 111 }.checker(invariant);
        assert_eq!(checker.check(100), CheckResult::Incomplete);
        assert_eq!(checker.sources().len(), 115); // not all generated were checked
        assert_eq!(
            checker.check(100_000),
            CheckResult::Fail { state: (Wrapping(3), Wrapping(15)) });
        assert_eq!(checker.sources().len(), 207); // only 187 were checked
    }

    #[test]
    fn model_check_can_resume_after_failing() {
        let mut checker = LinearEquation { a: 0, b: 0, c: 0 }.checker(invariant);
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
        let mut checker = LinearEquation { a: 2, b: 10, c: 14 }.checker(invariant);
        match checker.check(100_000) {
            CheckResult::Fail { state } => {
                assert_eq!(
                    checker.path_to(&state),
                    vec![
                        ("guess",      (Wrapping(0), Wrapping(0))),
                        ("increase x", (Wrapping(1), Wrapping(0))),
                        ("increase x", (Wrapping(2), Wrapping(0))),
                        ("increase y", (Wrapping(2), Wrapping(1))),
                    ]);
            },
            _ => panic!("expected solution")
        }
    }
}

