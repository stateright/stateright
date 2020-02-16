//! A model checker for a state machine.
//!
//! Models can have `sometimes` and `always` properties. The model checker will attempt to discover
//! an example of every `sometimes` property and a counterexample of every `always` property.
//! Usually the absence of a `sometimes` example or the presence of an `always` counterexample
//! indicates a problem with the implementation 
//!
//! An example that solves a [sliding puzzle](https://en.wikipedia.org/wiki/Sliding_puzzle)
//! follows.
//!
//! ```rust
//! use stateright::*;
//! use stateright::checker::*;
//!
//! #[derive(Clone, Debug, Eq, PartialEq)]
//! enum Slide { Down, Up, Right, Left }
//!
//! let puzzle = QuickMachine {
//!     init_states: || vec![vec![1, 4, 2,
//!                               3, 5, 8,
//!                               6, 7, 0]],
//!     actions: |_, actions| {
//!         actions.append(&mut vec![
//!             Slide::Down, Slide::Up, Slide::Right, Slide::Left
//!         ]);
//!     },
//!     next_state: |last_state, action| {
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
//!             let mut next_state = last_state.clone();
//!             next_state[empty] = last_state[from];
//!             next_state[from] = 0;
//!             next_state
//!         })
//!     }
//! };
//! let solved = vec![0, 1, 2,
//!                   3, 4, 5,
//!                   6, 7, 8];
//! let example = Model {
//!     state_machine: puzzle,
//!     properties: vec![Property::sometimes("solved", |_, state| { state == &solved })],
//!     boundary: None,
//! }.checker().check(100).example("solved");
//! assert_eq!(
//!     example,
//!     Some(Path(vec![
//!         (vec![1, 4, 2,
//!               3, 5, 8,
//!               6, 7, 0], Some(Slide::Down)),
//!         (vec![1, 4, 2,
//!               3, 5, 0,
//!               6, 7, 8], Some(Slide::Right)),
//!         (vec![1, 4, 2,
//!               3, 0, 5,
//!               6, 7, 8], Some(Slide::Down)),
//!         (vec![1, 0, 2,
//!               3, 4, 5,
//!               6, 7, 8], Some(Slide::Right)),
//!         (solved, None)])));
//! ```
//!
//! [Additional examples](https://github.com/stateright/stateright/tree/master/examples)
//! are available in the repository.

use crate::*;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;

/// A path of states including actions. i.e. `state --action--> state ... --action--> state`.
/// You can convert to a `Vec<_>` with `path.into_vec()`. If you only need the actions, then use
/// `path.into_actions()`.
#[derive(Debug, Eq, PartialEq)]
pub struct Path<State, Action>(pub Vec<(State, Option<Action>)>);
impl<State, Action> Path<State, Action> {
    /// Extracts the last state.
    pub fn last_state(&self) -> &State {
        &self.0.last().unwrap().0
    }
    /// Extracts the actions.
    pub fn into_actions(self) -> Vec<Action> {
        self.0.into_iter().filter_map(|(_s, a)| a).collect()
    }

    /// Convenience method for `Into<Vec<_>>`.
    pub fn into_vec(self) -> Vec<(State, Option<Action>)> {
        self.into()
    }
}
impl<State, Action> Into<Vec<(State, Option<Action>)>> for Path<State, Action> {
    fn into(self) -> Vec<(State, Option<Action>)> { self.0 }
}

/// A state machine model that can be checked.
pub struct Model<'a, SM: StateMachine> {
    pub state_machine: SM,
    pub properties: Vec<Property<'a, SM>>,
    pub boundary: Option<Box<dyn Fn(&SM, &SM::State) -> bool + Sync + 'a>>,
}
impl<'a, SM: StateMachine> Model<'a, SM> {
    /// Initializes a single-threaded model checker.
    pub fn checker(self) -> Checker<'a, SM> {
        self.checker_with_threads(1)
    }

    /// Initializes a model checker. The visitation order will be nondeterministic if
    /// `thread_count > 1`.
    pub fn checker_with_threads(self, thread_count: usize) -> Checker<'a, SM> {
        Checker {
            thread_count,
            model: self,
            pending: VecDeque::with_capacity(50_000),
            sources: DashMap::with_capacity(1_000_000),
            discoveries: DashMap::with_capacity(10),
        }
    }
}

/// A named predicate, such as "an epoch *sometimes* has no leader" (for which the the model
/// checker would find an example) or "an epoch *always* has at most one leader" (for which the
/// model checker would find a counterexample).
pub struct Property<'a, SM: StateMachine> {
    expectation: Expectation,
    name: &'static str,
    condition: Box<dyn Fn(&SM, &SM::State) -> bool + Sync + 'a>,
}
impl<'a, SM: StateMachine> Property<'a, SM> {
    /// An invariant that defines a [safety
    /// property](https://en.wikipedia.org/wiki/Safety_property). The model checker will try to
    /// discover a counterexample.
    pub fn always(name: &'static str, f: impl Fn(&SM, &SM::State) -> bool + Sync + 'a)
            -> Property<'a, SM> {
        Property { expectation: Expectation::Always, name, condition: Box::new(f) }
    }

    /// Something that should be possible in the model. The model checker will try to discover an
    /// example.
    pub fn sometimes(name: &'static str, f: impl Fn(&SM, &SM::State) -> bool + Sync + 'a)
            -> Property<'a, SM> {
        Property { expectation: Expectation::Sometimes, name, condition: Box::new(f) }
    }
}
#[derive(Debug, Eq, PartialEq)]
enum Expectation { Always, Sometimes }

/// Generates every state reachable by a state machine, and verifies that all properties hold.
/// Can be instantiated with `model.checker()`.
pub struct Checker<'a, SM: StateMachine> {
    thread_count: usize,
    model: Model<'a, SM>,
    pending: VecDeque<SM::State>,
    sources: DashMap<Fingerprint, Option<Fingerprint>>,
    discoveries: DashMap<&'static str, Fingerprint>,
}

impl<'a, SM: StateMachine> Checker<'a, SM>
where
    SM: Sync,
    SM::State: Debug + Hash + Send + Sync,
    SM::Action: Debug,
{
    /// Visits up to a specified number of states checking the model's properties. May return
    /// earlier when all states have been checked or all the properties are resolved.
    pub fn check(&mut self, max_count: usize) -> &mut Self {
        let Checker { thread_count, model, pending, sources, discoveries } = self;
        let thread_count = *thread_count; // mut ref -> owned copy
        if sources.is_empty() {
            for init_state in model.state_machine.init_states() {
                let init_digest = fingerprint(&init_state);
                if let Entry::Vacant(init_source) = sources.entry(init_digest) {
                    init_source.insert(None);
                    pending.push_front(init_state);
                }
            }
        }
        let sources = Arc::new(sources);
        let discoveries = Arc::new(discoveries);
        if pending.len() < 10_000 {
            Self::check_block(max_count, model, pending, sources, discoveries);
            return self;
        }
        let results = crossbeam_utils::thread::scope(|scope| {
            // 1. Kick off every thread.
            let mut threads = Vec::new();
            let count = std::cmp::min(max_count, (pending.len() + thread_count - 1) / thread_count);
            for _thread_id in 0..thread_count {
                let model = &model; // mut ref -> shared ref
                let sources = Arc::clone(&sources);
                let discoveries = Arc::clone(&discoveries);
                let count = std::cmp::min(pending.len(), count);
                let mut pending = pending.split_off(pending.len() - count);
                threads.push(scope.spawn(move |_| {
                    Self::check_block(max_count/thread_count, model, &mut pending, sources, discoveries);
                    pending
                }));
            }

            // 2. Join.
            let mut results = Vec::new();
            for thread in threads { results.push(thread.join().unwrap()); }
            results
        }).unwrap();

        // 3. Consolidate results.
        for mut sub_pending in results { pending.append(&mut sub_pending); }
        self
    }

    fn check_block(
            max_count: usize,
            model: &Model<SM>,
            pending: &mut VecDeque<SM::State>,
            sources: Arc<&mut DashMap<Fingerprint, Option<Fingerprint>>>,
            discoveries: Arc<&mut DashMap<&'static str, Fingerprint>>) {

        let state_machine = &model.state_machine;
        let mut remaining = max_count;
        let mut next_actions = Vec::new(); // reused between iterations for efficiency
        let mut properties: Vec<_> = Self::remaining_properties(&model.properties, &discoveries);

        if properties.is_empty() { return }

        while let Some(state) = pending.pop_back() {
            let digest = fingerprint(&state);

            // collect the next actions, and record the corresponding states that have not been
            // seen before if they are within the boundary
            state_machine.actions(&state, &mut next_actions);
            for next_action in next_actions.drain(0..) {
                if let Some(next_state) = state_machine.next_state(&state, next_action) {
                    if let Some(boundary) = &model.boundary {
                        if !boundary(state_machine, &next_state) { continue }
                    }

                    let next_digest = fingerprint(&next_state);
                    if let Entry::Vacant(next_entry) = sources.entry(next_digest) {
                        next_entry.insert(Some(digest));
                        pending.push_front(next_state);
                    }
                }
            }

            // check properties that lack associated discoveries
            let mut is_updated = false;
            for property in &properties {
                match property {
                    Property { expectation: Expectation::Always, name, condition: always } => {
                        if !always(&state_machine, &state) {
                            discoveries.insert(name, digest);
                            is_updated = true;
                        }
                    },
                    Property { expectation: Expectation::Sometimes, name, condition: sometimes } => {
                        if sometimes(&state_machine, &state) {
                            discoveries.insert(name, digest);
                            is_updated = true;
                        }
                    },
                }
            }
            if is_updated {
                properties = model.properties.iter().filter(|p| !discoveries.contains_key(p.name)).collect();
            }

            remaining -= 1;
            if remaining == 0 { return }
        }
    }

    /// An example of a "sometimes" property. `None` indicates that the property exists but no
    /// example has been found. Will panic if a corresponding "sometimes" property does not
    /// exist.
    pub fn example(&self, name: &'static str) -> Option<Path<SM::State, SM::Action>> {
        if let Some(p) = self.model.properties.iter().find(|p| p.name == name) {
            if p.expectation != Expectation::Sometimes {
                panic!("Please use `counterexample(\"{}\")` for this `always` property.", name);
            }
            self.discoveries.get(name).map(|mapref| self.path(*mapref.value()))
        } else {
            let available: Vec<_> = self.model.properties.iter().map(|p| p.name).collect();
            panic!("Unknown property. requested={}, available={:?}", name, available);
        }
    }

    /// A counterexaple of an "always" property. `None` indicates that the property exists but no
    /// counterexample has been found. Will panic if a corresponding "always" property does not
    /// exist.
    pub fn counterexample(&self, name: &'static str) -> Option<Path<SM::State, SM::Action>> {
        if let Some(p) = self.model.properties.iter().find(|p| p.name == name) {
            if p.expectation != Expectation::Always {
                panic!("Please use `example(\"{}\")` for this `sometimes` property.", name);
            }
            self.discoveries.get(name).map(|mapref| self.path(*mapref.value()))
        } else {
            let available: Vec<_> = self.model.properties.iter().map(|p| p.name).collect();
            panic!("Unknown property. requested={}, available={:?}", name, available);
        }
    }

    fn path(&self, fp: Fingerprint) -> Path<SM::State, SM::Action> {
        // First build a stack of digests representing the path (with the init digest at top of
        // stack). Then unwind the stack of digests into a vector of states. The TLC model checker
        // uses a similar technique, which is documented in the paper "Model Checking TLA+
        // Specifications" by Yu, Manolios, and Lamport.

        let state_machine = &self.model.state_machine;
        let sources = &self.sources;

        // 1. Build a stack of digests.
        let mut digests = Vec::new();
        let mut next_digest = fp;
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
        let init_states = state_machine.init_states();
        let mut last_state = init_states.into_iter()
            .find(|s| fingerprint(&s) == digests.pop().unwrap())
            .unwrap();

        // 3. Then continue with the remaining steps.
        let mut output = Vec::new();
        while let Some(next_digest) = digests.pop() {
            let mut actions = Vec::new();
            state_machine.actions(
                &last_state,
                &mut actions);

            let (action, next_state) = state_machine
                .next_steps(&last_state).into_iter()
                .find_map(|(a,s)| {
                    if fingerprint(&s) == next_digest {
                        Some((a, s))
                    } else {
                        None
                    }
                }).expect("state matching recorded digest");
            output.push((last_state, Some(action)));

            last_state = next_state;
        }
        output.push((last_state, None));
        Path(output)
    }

    /// Blocks the thread until model checking is complete. Periodically emits a status while
    /// checking, tailoring the block size to the checking speed. Emits a report when complete.
    pub fn check_and_report(&mut self, w: &mut impl std::io::Write) {
        use std::cmp::max;
        use std::time::Instant;

        let method_start = Instant::now();
        let mut block_size = 32_768;
        loop {
            let block_start = Instant::now();
            if self.check(block_size).is_done() {
                let elapsed = method_start.elapsed().as_secs();
                for mapref in self.discoveries.iter() {
                    let (name, fp) = mapref.pair();
                    writeln!(w, "== {} ==", name).unwrap();
                    for action in self.path(*fp).into_actions() {
                        writeln!(w, "ACTION: {:?}", action).unwrap();
                    }
                }
                writeln!(w, "Complete. generated={}, pending={}, sec={}",
                    self.sources.len(),
                    self.pending.len(),
                    elapsed
                ).unwrap();
                return;
            }

            let block_elapsed = block_start.elapsed().as_secs();
            if block_elapsed > 0 {
                println!("{} states pending after {} sec. Continuing.",
                         self.pending.len(),
                         method_start.elapsed().as_secs());
            }

            // Shrink or grow block if necessary.
            if block_elapsed < 2 { block_size = 3 * block_size / 2; }
            else if block_elapsed > 10 { block_size = max(1, block_size / 2); }
        }
    }

    /// Indicates that either all properties have associated discoveries or all reachable states
    /// have been visited.
    pub fn is_done(&'a self) -> bool {
        let remaining_properties = Self::remaining_properties(&self.model.properties, &self.discoveries);
        remaining_properties.is_empty() || self.pending.is_empty()
    }

    fn remaining_properties<'p, 'd>(
        properties: &'p [Property<'p, SM>],
        discoveries: &'d DashMap<&'static str, Fingerprint>
    ) -> Vec<&'p Property<'p, SM>> {
        properties.iter().filter(|p| !discoveries.contains_key(p.name)).collect()
    }

    /// Indicates how many states were generated during model checking.
    pub fn generated_count(&self) -> usize {
        self.sources.len()
    }

    /// Extracts the fingerprints generated during model checking.
    pub fn generated_fingerprints(&self) -> HashSet<Fingerprint> {
        self.sources.iter().map(|rm| *rm.key()).collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::*;

    #[test]
    fn records_states() {
        let h = |a: u8, b: u8| fingerprint(&(a, b));
        let state_space = LinearEquation { a: 2, b: 10, c: 14 }
            .model().checker()
            .check(100).generated_fingerprints();

        // Contains a variety of states.
        assert!(state_space.contains(&h(0, 0)));
        assert!(state_space.contains(&h(1, 0)));
        assert!(state_space.contains(&h(0, 1)));
        assert!(state_space.contains(&h(2, 0)));
        assert!(state_space.contains(&h(1, 1)));
        assert!(state_space.contains(&h(0, 2)));
        assert!(state_space.contains(&h(3, 0)));
        assert!(state_space.contains(&h(2, 1)));

        // And more generally, enumerates the entire block before returning.
        // Earlier versions of Stateright would return immediately after making a discovery.
        assert_eq!(state_space.len(), 115);
    }

    #[test]
    fn can_complete_by_enumerating_all_states() {
        let mut checker = LinearEquation { a: 2, b: 4, c: 7 }.model().checker();

        // Not solved, but not done.
        assert_eq!(checker.check(10).example("solvable"), None);
        assert_eq!(checker.is_done(), false);

        // Sources is larger than the check size because it includes pending.
        assert_eq!(checker.pending.len(), 5); 
        assert_eq!(checker.sources.len(), 15); 

        // Not solved, and done checking, so no solution in the domain.
        assert_eq!(checker.check(100_000).is_done(), true);
        assert_eq!(checker.example("solvable"), None);

        // Now sources is less than the check size (256^2 = 65,536).
        assert_eq!(checker.sources.len(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let mut checker = LinearEquation { a: 1, b: 2, c: 3 }.model().checker();

        // Solved and done (with example identified) ...
        assert!(checker.check(100).is_done());
        assert_eq!(checker.example("solvable"), Some(Path(vec![
            ((0, 0), Some(Guess::IncreaseX)),
            ((1, 0), Some(Guess::IncreaseY)),
            ((1, 1), None),
        ])));

        // but didn't need to enumerate all of state space...
        assert_eq!(checker.pending.len(), 15); 
        assert_eq!(checker.sources.len(), 115); 

        // and won't check any more states since it's done.
        checker.check(100);
        assert_eq!(checker.pending.len(), 15); 
        assert_eq!(checker.sources.len(), 115);
    }

    #[test]
    fn report_includes_property_names_and_paths() {
        let mut written: Vec<u8> = Vec::new();
        LinearEquation { a: 2, b: 10, c: 14 }.model().checker().check_and_report(&mut written);
        let output = String::from_utf8(written).unwrap();
        // `starts_with` to omit timing since it varies
        assert!(
            output.starts_with("\
                == solvable ==\n\
                ACTION: IncreaseX\n\
                ACTION: IncreaseX\n\
                ACTION: IncreaseY\n\
                Complete. generated=33024, pending=256, sec="),
            "Output did not start as expected (see test). output={:?}`", output);
    }
}
