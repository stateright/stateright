//! A `Model` checker.
//!
//! Models can have `sometimes` and `always` properties. The model checker will attempt to discover
//! an example of every `sometimes` property and a counterexample of every `always` property.
//! Usually the absence of a `sometimes` example or the presence of an `always` counterexample
//! indicates a problem with the implementation.
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
//! struct Puzzle(Vec<u8>);
//! impl Model for Puzzle {
//!     type State = Vec<u8>;
//!     type Action = Slide;
//!
//!     fn init_states(&self) -> Vec<Self::State> {
//!         vec![self.0.clone()]
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
//!             let mut next_state = last_state.clone();
//!             next_state[empty] = last_state[from];
//!             next_state[from] = 0;
//!             next_state
//!         })
//!     }
//!
//!     fn properties(&self) -> Vec<Property<Self>> {
//!         vec![Property::sometimes("solved", |_, state: &Vec<u8>| {
//!             let solved = vec![0, 1, 2,
//!                               3, 4, 5,
//!                               6, 7, 8];
//!             state == &solved
//!         })]
//!     }
//! }
//! let example = Puzzle(vec![1, 4, 2,
//!                           3, 5, 8,
//!                           6, 7, 0])
//!     .checker().check(100).assert_example("solved");
//! assert_eq!(
//!     example,
//!     Path(vec![
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
//!         (vec![0, 1, 2,
//!               3, 4, 5,
//!               6, 7, 8], None)]));
//! ```
//!
//! [Additional examples](https://github.com/stateright/stateright/tree/master/examples)
//! are available in the repository.

use crate::*;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use id_set::IdSet;
use nohash_hasher::NoHashHasher;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::{BuildHasher, BuildHasherDefault};
use std::sync::Arc;

/// A path of states including actions. i.e. `state --action--> state ... --action--> state`.
/// You can convert to a `Vec<_>` with `path.into_vec()`. If you only need the actions, then use
/// `path.into_actions()`.
#[derive(Clone, Debug, PartialEq)]
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

    pub fn name(&self) -> PathName where State: Hash {
        self.0.iter()
            .map(|(s, _a)| format!("{}", fingerprint(s)))
            .collect::<Vec<String>>()
            .join("/")
    }
}
impl<State, Action> Into<Vec<(State, Option<Action>)>> for Path<State, Action> {
    fn into(self) -> Vec<(State, Option<Action>)> { self.0 }
}

/// An identifier that fully qualifies a path.
pub type PathName = String;

/// EventuallyBits tracks one bit per 'eventually' property being checked. Properties are assigned
/// bit-numbers just by counting the 'eventually' properties up from 0 in the properties list. If a
/// bit is present in a bitset, the property has _not_ been found on this path yet. Bits are removed
/// from the propagating bitset when we find a state satisfying an `eventually` property; these
/// states are not considered discoveries. Only if we hit the "end" of a path (i.e. return to a known
/// state / no further state) with any of these bits still 1, the path is considered a discovery,
/// a counterexample to the property.
type EventuallyBits = IdSet;

/// Generates every state reachable by a model, and verifies that all properties hold.
/// Can be instantiated with `Model::checker()` or `Model::checker_with_threads(...)`.
pub struct Checker<M: Model> {
    pub(crate) thread_count: usize,
    pub(crate) model: M,
    pub(crate) pending: VecDeque<(Fingerprint, EventuallyBits, M::State)>,
    pub(crate) sources: DashMap<Fingerprint, Option<Fingerprint>, BuildHasherDefault<NoHashHasher<u64>>>,
    pub(crate) discoveries: DashMap<&'static str, Fingerprint>,
}

impl<M: Model> Checker<M>
where
    M: Sync,
    M::State: Debug + Hash + Send + Sync,
    M::Action: Debug,
{
    /// Visits up to a specified number of states checking the model's properties. May return
    /// earlier when all states have been checked or all the properties are resolved.
    pub fn check(&mut self, max_count: usize) -> &mut Self {
        let Checker { thread_count, model, pending, sources, discoveries } = self;
        let thread_count = *thread_count; // mut ref -> owned copy
        if sources.is_empty() {
            let mut init_ebits = IdSet::new();
            for (i, p) in model.properties().iter().enumerate() {
                if let Property { expectation: Expectation::Eventually, .. } = p {
                    init_ebits.insert(i);
                }
            }
            for init_state in model.init_states() {
                let init_digest = fingerprint(&init_state);
                if let Entry::Vacant(init_source) = sources.entry(init_digest) {
                    init_source.insert(None);
                    pending.push_front((init_digest, init_ebits.clone(), init_state));
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

    // Removes from ebits the bit-number of any 'eventually' property that
    // holds in state, leaving only bits that (still) _don't_ hold.
    fn check_properties(model: &M,
                        state: &M::State,
                        digest: Fingerprint,
                        props: &Vec<Property<M>>,
                        ebits: &mut EventuallyBits,
                        discoveries: &DashMap<&'static str, Fingerprint>,
                        update_props: &mut bool)
    {
        for (i, property) in props.iter().enumerate() {
            match property {
                Property { expectation: Expectation::Always, name, condition: always } => {
                    if !always(model, &state) {
                        discoveries.insert(name, digest);
                        *update_props = true;
                    }
                },
                Property { expectation: Expectation::Sometimes, name, condition: sometimes } => {
                    if sometimes(model, &state) {
                        discoveries.insert(name, digest);
                        *update_props = true;
                    }
                },
                Property { expectation: Expectation::Eventually, condition, .. } => {
                    if ebits.contains(i) && condition(model, &state) {
                        ebits.remove(i);
                        *update_props = true;
                    }
                }
            }
        }
    }

    fn note_terminal_state(digest: Fingerprint,
                           props: &Vec<Property<M>>,
                           ebits: &EventuallyBits,
                           discoveries: &DashMap<&'static str, Fingerprint>,
                           update_props: &mut bool)
    {
        for (i, property) in props.iter().enumerate() {
            if ebits.contains(i) {
                discoveries.insert(property.name, digest);
                *update_props = true;
            }
        }
    }

    fn check_block<S: Clone + BuildHasher>(
        max_count: usize,
        model: &M,
        pending: &mut VecDeque<(Fingerprint, EventuallyBits, M::State)>,
        sources: Arc<&mut DashMap<Fingerprint, Option<Fingerprint>, S>>,
        discoveries: Arc<&mut DashMap<&'static str, Fingerprint>>) {

        let mut remaining = max_count;
        let mut next_actions = Vec::new(); // reused between iterations for efficiency
        let mut properties: Vec<_> = Self::remaining_properties(
            model.properties(), &discoveries);

        if properties.is_empty() { return }

        while let Some((digest, mut ebits, state)) = pending.pop_back() {
            let mut update_props = false;
            Self::check_properties(model, &state, digest, &properties,
                                   &mut ebits, &discoveries, &mut update_props);

            // collect the next actions, and record the corresponding states that have not been
            // seen before if they are within the boundary
            model.actions(&state, &mut next_actions);
            if next_actions.is_empty() {
                // No actions implies "terminal state".
                Self::note_terminal_state(digest, &properties, &ebits,
                                          &discoveries, &mut update_props);
            }
            for next_action in next_actions.drain(0..) {
                if let Some(next_state) = model.next_state(&state, next_action) {
                    if !model.within_boundary(&next_state) {
                        // Boundary implies "terminal state".
                        Self::note_terminal_state(digest, &properties, &ebits,
                                                  &discoveries, &mut update_props);
                        continue
                    }

                    // FIXME: we should really include ebits in the fingerprint here --
                    // it is possible to arrive at a DAG join with two different ebits
                    // values, and subsequently treat the fact that some eventually
                    // property held on the path leading to the first visit as meaning
                    // that it holds in the path leading to the second visit -- another
                    // possible false-negative.
                    let next_digest = fingerprint(&next_state);
                    if let Entry::Vacant(next_entry) = sources.entry(next_digest) {
                        next_entry.insert(Some(digest));
                        pending.push_front((next_digest, ebits.clone(), next_state));
                    } else {
                        // FIXME: arriving at an already-known state may be a loop (in which case it
                        // could, in a fancier implementation, be considered a terminal state for
                        // purposes of eventually-property checking) but it might also be a join in
                        // a DAG, which makes it non-terminal. These cases can be disambiguated (at
                        // some cost), but for now we just _don't_ treat them as terminal, and tell
                        // users they need to explicitly ensure model path-acyclicality when they're
                        // using eventually properties (using a boundary or empty actions or
                        // whatever).
                    }
                } else {
                    // No next-state implies "terminal state".
                    Self::note_terminal_state(digest, &properties, &ebits,
                                              &discoveries, &mut update_props);
                }
            }

            if update_props {
                properties = properties.into_iter().filter(|p| !discoveries.contains_key(p.name)).collect();
            }

            remaining -= 1;
            if remaining == 0 { return }
        }
    }

    /// An example of a "sometimes" property. `None` indicates that the property exists but no
    /// example has been found. Will panic if a corresponding "sometimes" property does not
    /// exist.
    pub fn example(&self, name: &'static str) -> Option<Path<M::State, M::Action>> {
        if let Some(p) = self.model.properties().into_iter().find(|p| p.name == name) {
            if p.expectation != Expectation::Sometimes {
                panic!("Please use `counterexample(\"{}\")` for this `always` or `eventually` property.", name);
            }
            self.discoveries.get(name).map(|mapref| self.path(*mapref.value()))
        } else {
            let available: Vec<_> = self.model.properties().iter().map(|p| p.name).collect();
            panic!("Unknown property. requested={}, available={:?}", name, available);
        }
    }

    /// A counterexample of an "always" or "eventually" property. `None` indicates that the property exists but no
    /// counterexample has been found. Will panic if a corresponding "always" or "eventually" property does not
    /// exist.
    pub fn counterexample(&self, name: &'static str) -> Option<Path<M::State, M::Action>> {
        if let Some(p) = self.model.properties().iter().find(|p| p.name == name) {
            if p.expectation == Expectation::Sometimes {
                panic!("Please use `example(\"{}\")` for this `sometimes` property.", name);
            }
            self.discoveries.get(name).map(|mapref| self.path(*mapref.value()))
        } else {
            let available: Vec<_> = self.model.properties().iter().map(|p| p.name).collect();
            panic!("Unknown property. requested={}, available={:?}", name, available);
        }
    }

    pub fn path(&self, fp: Fingerprint) -> Path<M::State, M::Action> {
        // First build a stack of digests representing the path (with the init digest at top of
        // stack). Then unwind the stack of digests into a vector of states. The TLC model checker
        // uses a similar technique, which is documented in the paper "Model Checking TLA+
        // Specifications" by Yu, Manolios, and Lamport.

        let model = &self.model;
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
        let init_states = model.init_states();
        let mut last_state = init_states.into_iter()
            .find(|s| fingerprint(&s) == digests.pop().unwrap())
            .unwrap();

        // 3. Then continue with the remaining steps.
        let mut output = Vec::new();
        while let Some(next_digest) = digests.pop() {
            let mut actions = Vec::new();
            model.actions(
                &last_state,
                &mut actions);

            let (action, next_state) = model
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
    pub fn is_done(&self) -> bool {
        let remaining_properties = Self::remaining_properties(self.model.properties(), &self.discoveries);
        remaining_properties.is_empty() || self.pending.is_empty()
    }

    fn remaining_properties(
        properties: Vec<Property<M>>,
        discoveries: &DashMap<&str, Fingerprint>
    ) -> Vec<Property<M>> {
        properties.into_iter().filter(|p| !discoveries.contains_key(p.name)).collect()
    }

    /// Indicates how many states were generated during model checking.
    pub fn generated_count(&self) -> usize {
        self.sources.len()
    }

    /// Extracts the fingerprints generated during model checking.
    pub fn generated_fingerprints(&self) -> HashSet<Fingerprint> {
        self.sources.iter().map(|rm| *rm.key()).collect()
    }

    /// A helper that verifies examples exist for all `sometimes` properties and no counterexamples
    /// exist for any `always`/`eventually` properties.
    pub fn assert_properties(&self) -> &Self {
        for p in self.model.properties() {
            match p.expectation {
                Expectation::Always => self.assert_no_counterexample(p.name),
                Expectation::Eventually => self.assert_no_counterexample(p.name),
                Expectation::Sometimes => { self.assert_example(p.name); },
            }
        }
        self
    }

    /// Panics if an example is not found. Otherwise returns a path to the example.
    pub fn assert_example(&self, name: &'static str) -> Path<M::State, M::Action> {
        if let Some(path) = self.example(name) {
            return path;
        }
        assert!(self.is_done(),
                "Example for '{}' not found, but model checking is incomplete.");
        panic!("Example for '{}' not found. `stateright::explorer` may be useful for debugging.", name);
    }

    /// Panics if a counterexample is not found. Otherwise returns a path to the counterexample.
    pub fn assert_counterexample(&self, name: &'static str) -> Path<M::State, M::Action> {
        if let Some(path) = self.counterexample(name) {
            return path;
        }
        assert!(self.is_done(),
                "Counterexample for '{}' not found, but model checking is incomplete.");
        panic!("Counterexample for '{}' not found. `stateright::explorer` may be useful for debugging.", name);
    }

    /// Panics if an example is found.
    pub fn assert_no_example(&self, name: &'static str) {
        if let Some(example) = self.example(name) {
            let last_state = format!("{:#?}", example.last_state());
            let actions = example.into_actions()
                .iter()
                .map(|a| format!("{:?}", a))
                .collect::<Vec<_>>()
                .join("\n");
            panic!("Example for '{}' found.\n\n== ACTIONS ==\n{}\n\n== LAST STATE ==\n{}",
                   name, actions, last_state);
        }
        assert!(self.is_done(),
                "Example for '{}' not found, but model checking is incomplete.",
                name);
    }

    /// Panics if a counterexample is found.
    pub fn assert_no_counterexample(&self, name: &'static str) {
        if let Some(counterexample) = self.counterexample(name) {
            let last_state = format!("{:#?}", counterexample.last_state());
            let actions = counterexample.into_actions()
                .iter()
                .map(|a| format!("{:?}", a))
                .collect::<Vec<_>>()
                .join("\n");
            panic!("Counterexample for '{}' found.\n\n== ACTIONS ==\n{}\n\n== LAST STATE ==\n{}",
                   name, actions, last_state);
        }
        assert!(self.is_done(),
                "Counterexample for '{}' not found, but model checking is incomplete.",
                name);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::*;

    #[test]
    fn records_states() {
        let h = |a: u8, b: u8| fingerprint(&(a, b));
        let state_space = LinearEquation { a: 2, b: 10, c: 14 }.checker()
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
        let mut checker = LinearEquation { a: 2, b: 4, c: 7 }.checker();

        // Not solved, but not done.
        assert_eq!(checker.check(10).example("solvable"), None);
        assert_eq!(checker.is_done(), false);

        // Sources is larger than the check size because it includes pending.
        assert_eq!(checker.pending.len(), 5); 
        assert_eq!(checker.sources.len(), 15); 

        // Not solved, and done checking, so no solution in the domain.
        assert_eq!(checker.check(100_000).is_done(), true);
        checker.assert_no_example("solvable");

        // Now sources is less than the check size (256^2 = 65,536).
        assert_eq!(checker.sources.len(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let mut checker = LinearEquation { a: 1, b: 2, c: 3 }.checker();

        // Solved and done (with example identified) ...
        assert_eq!(
            checker.check(100).assert_example("solvable"),
            Path(vec![
                ((0, 0), Some(Guess::IncreaseX)),
                ((1, 0), Some(Guess::IncreaseY)),
                ((1, 1), None),
            ]));

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
        LinearEquation { a: 2, b: 10, c: 14 }.checker().check_and_report(&mut written);
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
