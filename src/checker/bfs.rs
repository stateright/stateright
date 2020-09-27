//! Private module for selective re-export.

use core::hash::Hash;
use crate::{fingerprint, Fingerprint, Model, Property};
use crate::checker::{Expectation, ModelChecker, Path};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use id_set::IdSet;
use nohash_hasher::NoHashHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, BuildHasherDefault};

/// Generates every state reachable by a model, and verifies that all properties hold.
/// Can be instantiated with [`Model::checker`] or [`Model::checker_with_threads`].
pub struct BfsChecker<M: Model> {
    thread_count: usize,
    model: M,
    pending: VecDeque<(Fingerprint, EventuallyBits, M::State)>,
    sources: DashMap<Fingerprint, Option<Fingerprint>, BuildHasherDefault<NoHashHasher<u64>>>,
    discoveries: DashMap<&'static str, Fingerprint>,
}

/// EventuallyBits tracks one bit per 'eventually' property being checked. Properties are assigned
/// bit-numbers just by counting the 'eventually' properties up from 0 in the properties list. If a
/// bit is present in a bitset, the property has _not_ been found on this path yet. Bits are removed
/// from the propagating bitset when we find a state satisfying an `eventually` property; these
/// states are not considered discoveries. Only if we hit the "end" of a path (i.e. return to a known
/// state / no further state) with any of these bits still 1, the path is considered a discovery,
/// a counterexample to the property.
type EventuallyBits = IdSet;

impl<M> ModelChecker<M> for BfsChecker<M>
where M: Model + Sync,
      M::State: Hash + Send + Sync,
{
    fn model(&self) -> &M { &self.model }

    fn generated_count(&self) -> usize { self.sources.len() }

    fn pending_count(&self) -> usize { self.pending.len() }

    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>> {
        self.model().properties().into_iter()
            .filter_map(|p| {
                self.discoveries.get(p.name)
                    .map(|d| (p.name, self.path(*d)))
            })
            .collect()
    }

    fn check(&mut self, max_count: usize) -> &mut Self {
        let Self { thread_count, model, pending, sources, discoveries } = self;
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
        let sources = &sources;
        let discoveries = &discoveries;
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
}

impl<M> BfsChecker<M>
where M: Model,
{
    /// Instantiate a [`BfsChecker`].
    /// 
    /// Tip: use [`BfsChecker::with_threads`] for faster checking on multicore
    /// machines.
    pub fn new(model: M) -> Self {
        Self {
            thread_count: 1,
            model,
            pending: VecDeque::with_capacity(50_000),
            sources: DashMap::with_capacity_and_hasher(1_000_000, Default::default()),
            discoveries: DashMap::with_capacity(10),
        }
    }

    /// Sets the number of threads to use for model checking. The visitation order
    /// will be nondeterministic if `thread_count > 1`.
    pub fn with_threads(self, thread_count: usize) -> Self {
        Self { thread_count, .. self }
    }

    /// Extracts the fingerprints generated during model checking.
    #[cfg(test)]
    pub(crate) fn generated_fingerprints(&self) -> std::collections::HashSet<Fingerprint> {
        self.sources.iter().map(|rm| *rm.key()).collect()
    }
}

impl<M> BfsChecker<M>
where M: Model,
      M::State: Hash,
{
    fn check_block<S: Clone + BuildHasher>(
        max_count: usize,
        model: &M,
        pending: &mut VecDeque<(Fingerprint, EventuallyBits, M::State)>,
        sources: &DashMap<Fingerprint, Option<Fingerprint>, S>,
        discoveries: &DashMap<&'static str, Fingerprint>) {

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

    fn remaining_properties(
        properties: Vec<Property<M>>,
        discoveries: &DashMap<&str, Fingerprint>
    ) -> Vec<Property<M>> {
        properties.into_iter().filter(|p| !discoveries.contains_key(p.name)).collect()
    }

    // Removes from ebits the bit-number of any 'eventually' property that
    // holds in state, leaving only bits that (still) _don't_ hold.
    fn check_properties(model: &M,
                        state: &M::State,
                        digest: Fingerprint,
                        props: &[Property<M>],
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
                           props: &[Property<M>],
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

    fn path(&self, fp: Fingerprint) -> Path<M::State, M::Action> {
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
        assert_eq!(checker.pending_count(), 5); 
        assert_eq!(checker.generated_count(), 15); 

        // Not solved, and done checking, so no solution in the domain.
        assert_eq!(checker.check(100_000).is_done(), true);
        checker.assert_no_example("solvable");

        // Now sources is less than the check size (256^2 = 65,536).
        assert_eq!(checker.generated_count(), 256 * 256);
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
        assert_eq!(checker.pending_count(), 15); 
        assert_eq!(checker.generated_count(), 115); 

        // and won't check any more states since it's done.
        checker.check(100);
        assert_eq!(checker.pending_count(), 15); 
        assert_eq!(checker.generated_count(), 115);
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
