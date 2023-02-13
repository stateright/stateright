//! Private module for selective re-export.

use crate::{CheckerBuilder, CheckerVisitor, Fingerprint, fingerprint, Model, Property};
use crate::checker::{Checker, EventuallyBits, Expectation, Path};
use dashmap::DashMap;
use nohash_hasher::NoHashHasher;
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hash};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// While this file is currently quite similar to bfs.rs, a refactoring to lift shared
// behavior is being postponed until DPOR is implemented.

pub(crate) struct DfsChecker<M: Model> {
    // Immutable state.
    model: Arc<M>,
    thread_count: usize,
    handles: Vec<std::thread::JoinHandle<()>>,

    // Mutable state.
    job_market: Arc<Mutex<JobMarket<M::State>>>,
    state_count: Arc<AtomicUsize>,
    max_depth: Arc<AtomicUsize>,
    // total degree, to be divided by total number of states to get the average degree per state
    total_out_degree: Arc<AtomicUsize>,
    // mapping from fingerprints to in-degree for that state
    generated: Arc<DashMap<Fingerprint, usize, BuildHasherDefault<NoHashHasher<u64>>>>,
    discoveries: Arc<DashMap<&'static str, Vec<Fingerprint>>>,
}
struct JobMarket<State> { wait_count: usize, jobs: Vec<Job<State>> }
type Job<State> = Vec<(State, Vec<Fingerprint>, EventuallyBits, NonZeroUsize)>;

impl<M> DfsChecker<M>
where M: Model + Send + Sync + 'static,
      M::State: Hash + Send + 'static,
{
    pub(crate) fn spawn(options: CheckerBuilder<M>) -> Self {
        let model = Arc::new(options.model);
        let symmetry = options.symmetry;
        let target_state_count = options.target_state_count;
        let target_max_depth = options.target_max_depth;
        let thread_count = options.thread_count;
        let visitor = Arc::new(options.visitor);
        let property_count = model.properties().len();

        let init_states: Vec<_> = model.init_states().into_iter()
            .filter(|s| model.within_boundary(s))
            .collect();
        let state_count = Arc::new(AtomicUsize::new(init_states.len()));
        let max_depth = Arc::new(AtomicUsize::new(0));
        let total_out_degree = Arc::new(AtomicUsize::new(0));
        let generated = Arc::new({
            let generated = DashMap::default();
            for s in &init_states {
                if let Some(representative) = symmetry {
                    generated.insert(fingerprint(&representative(s)), 0);
                } else {
                    generated.insert(fingerprint(s), 0);
                }
            }
            generated
        });
        let ebits = {
            let mut ebits = EventuallyBits::new();
            for (i, p) in model.properties().iter().enumerate() {
                if let Property { expectation: Expectation::Eventually, .. } = p {
                    ebits.insert(i);
                }
            }
            ebits
        };
        let pending: Vec<_> = init_states.into_iter()
            .map(|s| {
                let fp = fingerprint(&s);
                (s, vec![fp], ebits.clone(), NonZeroUsize::new(1).unwrap())
            })
            .collect();
        let discoveries = Arc::new(DashMap::default());
        let mut handles = Vec::new();

        let has_new_job = Arc::new(Condvar::new());
        let job_market = Arc::new(Mutex::new(JobMarket {
            wait_count: thread_count,
            jobs: vec![pending],
        }));
        for t in 0..thread_count {
            let model = Arc::clone(&model);
            let visitor = Arc::clone(&visitor);
            let has_new_job = Arc::clone(&has_new_job);
            let job_market = Arc::clone(&job_market);
            let state_count = Arc::clone(&state_count);
            let max_depth = Arc::clone(&max_depth);
            let total_out_degree = Arc::clone(&total_out_degree);
            let generated = Arc::clone(&generated);
            let discoveries = Arc::clone(&discoveries);
            handles.push(std::thread::Builder::new()
                .name(format!("checker-{}", t))
                .spawn(move || {
                log::debug!("{}: Thread started.", t);
                let mut pending = Vec::new();
                loop {
                    // Step 1: Do work.
                    if pending.is_empty() {
                        pending = {
                            let mut job_market = job_market.lock();
                            match job_market.jobs.pop() {
                                None => {
                                    // Done if all are waiting.
                                    if job_market.wait_count == thread_count {
                                        log::debug!("{}: No more work. Shutting down... gen={}", t, generated.len());
                                        has_new_job.notify_all();
                                        return
                                    }

                                    // Otherwise more work may become available.
                                    log::trace!("{}: No jobs. Awaiting. blocked={}", t, job_market.wait_count);
                                    has_new_job.wait(&mut job_market);
                                    continue
                                }
                                Some(job) => {
                                    job_market.wait_count -= 1;
                                    log::trace!("{}: Job found. size={}, blocked={}", t, job.len(), job_market.wait_count);
                                    job
                                }
                            }
                        };
                    }
                    Self::check_block(
                        &*model,
                        &*state_count,
                        &*generated,
                        &mut pending,
                        &*discoveries,
                        &*visitor,
                        1500,
                        target_max_depth,
                        &max_depth,
                        &total_out_degree,
                        symmetry,
                    );
                    if discoveries.len() == property_count {
                        log::debug!("{}: Discovery complete. Shutting down... gen={}", t, generated.len());
                        let mut job_market = job_market.lock();
                        job_market.wait_count += 1;
                        drop(job_market);
                        has_new_job.notify_all();
                        return
                    }
                    if let Some(target_state_count) = target_state_count {
                        if target_state_count.get() <= state_count.load(Ordering::Relaxed) {
                            log::debug!("{}: Reached target state count. Shutting down... gen={}",
                                        t, generated.len());
                            return;
                        }
                    }

                    // Step 2: Share work.
                    if pending.len() > 1 && thread_count > 1 {
                        let mut job_market = job_market.lock();
                        let pieces = 1 + std::cmp::min(job_market.wait_count as usize, pending.len());
                        let size = pending.len() / pieces;
                        for _ in 1..pieces {
                            log::trace!("{}: Sharing work. blocked={}, size={}", t, job_market.wait_count, size);
                            job_market.jobs.push(pending.split_off(pending.len() - size));
                            has_new_job.notify_one();
                        }
                    } else if pending.is_empty() {
                        let mut job_market = job_market.lock();
                        job_market.wait_count += 1;
                    }
                }
            }).expect("Failed to spawn a thread"));
        }
        DfsChecker {
            model,
            thread_count,
            handles,
            job_market,
            state_count,
            max_depth,
            total_out_degree,
            generated,
            discoveries,
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    fn check_block(
        model: &M,
        state_count: &AtomicUsize,
        generated: &DashMap<Fingerprint, usize, BuildHasherDefault<NoHashHasher<u64>>>,
        pending: &mut Job<M::State>,
        discoveries: &DashMap<&'static str, Vec<Fingerprint>>,
        visitor: &Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
        mut max_count: usize,
        target_max_depth: Option<NonZeroUsize>,
        global_max_depth: &AtomicUsize,
        total_out_degree: &AtomicUsize,
        symmetry: Option<fn(&M::State) -> M::State>,
    ) {
        let properties = model.properties();

        let mut current_max_depth = global_max_depth.load(Ordering::Relaxed);
        let mut actions = Vec::new();
        loop {
            // Done if reached max count.
            if max_count == 0 { return }
            max_count -= 1;

            // Done if none pending.
            let (state, fingerprints, mut ebits, max_depth) = match pending.pop() {
                None => return,
                Some(pair) => pair,
            };

            if max_depth.get() > current_max_depth {
                let _ = global_max_depth.compare_exchange(current_max_depth, max_depth.get(), Ordering::Relaxed, Ordering::Relaxed);
                current_max_depth = max_depth.get();
            }

            if let Some(target_max_depth) = target_max_depth {
                if max_depth >= target_max_depth {
                    log::trace!("Skipping state as past max depth {}", max_depth);
                    continue;
                }
            }
            if let Some(visitor) = visitor {
                visitor.visit(model, Path::from_fingerprints(
                        model,
                        VecDeque::from(fingerprints.clone())));
            }

            // Done if discoveries found for all properties.
            let mut is_awaiting_discoveries = false;
            for (i, property) in properties.iter().enumerate() {
                if discoveries.contains_key(property.name) { continue }
                match property {
                    Property { expectation: Expectation::Always, condition: always, .. } => {
                        if !always(model, &state) {
                            // Races other threads, but that's fine.
                            discoveries.insert(property.name, fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    },
                    Property { expectation: Expectation::Sometimes, condition: sometimes, .. } => {
                        if sometimes(model, &state) {
                            // Races other threads, but that's fine.
                            discoveries.insert(property.name, fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    },
                    Property { expectation: Expectation::Eventually, condition: eventually, .. } => {
                        // The checker early exits after finding discoveries for every property,
                        // and "eventually" property discoveries are only identifid at terminal
                        // states, so if we are here it means we are still awaiting a corresponding
                        // discovery regardless of whether the eventually property is now satisfied
                        // (i.e. it might be falsifiable via a different path).
                        is_awaiting_discoveries = true;
                        if eventually(model, &state) {
                            ebits.remove(i);
                        }
                    }
                }
            }
            if !is_awaiting_discoveries { return }

            // Otherwise enqueue newly generated states (with related metadata).
            let mut is_terminal = true;
            model.actions(&state, &mut actions);
            total_out_degree.fetch_add(actions.len(), Ordering::Relaxed);
            for action in actions.drain(..) {
                let next_state = match model.next_state(&state, action) {
                    None => continue,
                    Some(next_state) => next_state,
                };

                // Skip if outside boundary.
                if !model.within_boundary(&next_state) { continue }
                state_count.fetch_add(1, Ordering::Relaxed);

                // Skip if already generated.
                //
                // FIXME: we should really include ebits in the fingerprint here --
                // it is possible to arrive at a DAG join with two different ebits
                // values, and subsequently treat the fact that some eventually
                // property held on the path leading to the first visit as meaning
                // that it holds in the path leading to the second visit -- another
                // possible false-negative.
                let next_fingerprint = if let Some(representative) = symmetry {
                    let representative_fingerprint = fingerprint(&representative(&next_state));
                    match generated.entry(representative_fingerprint) {
                        dashmap::mapref::entry::Entry::Occupied(mut o) =>  {
                            o.insert(o.get() + 1);
                            is_terminal = false;
                            continue
                        },
                        dashmap::mapref::entry::Entry::Vacant(v) => {
                            v.insert(1);
                        },
                    }
                    // IMPORTANT: continue the path with the pre-canonicalized state/fingerprint to
                    // avoid jumping to another part of the state space for which there may not be
                    // a path extension from the previously collected path.
                    fingerprint(&next_state)
                } else {
                    let next_fingerprint = fingerprint(&next_state);
                    match generated.entry(next_fingerprint) {
                        dashmap::mapref::entry::Entry::Occupied(mut o) => {
                            o.insert(o.get() + 1);
                            // FIXME: arriving at an already-known state may be a loop (in which case it
                            // could, in a fancier implementation, be considered a terminal state for
                            // purposes of eventually-property checking) but it might also be a join in
                            // a DAG, which makes it non-terminal. These cases can be disambiguated (at
                            // some cost), but for now we just _don't_ treat them as terminal, and tell
                            // users they need to explicitly ensure model path-acyclicality when they're
                            // using eventually properties (using a boundary or empty actions or
                            // whatever).
                            is_terminal = false;
                            continue
                        },
                        dashmap::mapref::entry::Entry::Vacant(v) => {
                            v.insert(1);
                        },
                    }
                    next_fingerprint
                };

                // Otherwise further checking is applicable.
                is_terminal = false;
                let mut next_fingerprints = Vec::with_capacity(1 + fingerprints.len());
                for f in &fingerprints { next_fingerprints.push(*f); }
                next_fingerprints.push(next_fingerprint);
                pending.push((
                    next_state,
                    next_fingerprints,
                    ebits.clone(),
                    NonZeroUsize::new(max_depth.get() + 1).unwrap(),
                ));
            }
            if is_terminal {
                for (i, property) in properties.iter().enumerate() {
                    if ebits.contains(i) {
                        // Races other threads, but that's fine.
                        discoveries.insert(property.name, fingerprints.clone());
                    }
                }
            }
        }
    }
}

impl<M> Checker<M> for DfsChecker<M>
where M: Model,
      M::State: Hash,
{
    fn model(&self) -> &M { &self.model }

    fn state_count(&self) -> usize {
        self.state_count.load(Ordering::Relaxed)
    }

    fn unique_state_count(&self) -> usize { self.generated.len() }

    fn max_depth(&self) -> usize {
        self.max_depth.load(Ordering::Relaxed)
    }

    fn average_out_degree(&self) -> f64 {
        let total = self.total_out_degree.load(Ordering::Relaxed);
        let state_count = self.state_count();
        total as f64 / state_count as f64
    }

    fn average_in_degree(&self) -> f64 {
        let total: usize = self.generated.iter().map(|kv| *kv.value()).sum();
        let state_count = self.unique_state_count();
        total as f64 / state_count as f64
    }

    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>> {
        self.discoveries.iter()
            .map(|mapref| {
                (
                    <&'static str>::clone(mapref.key()),
                    Path::from_fingerprints(
                        self.model(),
                        VecDeque::from(mapref.value().clone())),
                )
            })
            .collect()
    }

    fn join(mut self) -> Self {
        for h in self.handles.drain(0..) {
            h.join().unwrap();
        }
        self
    }

    fn is_done(&self) -> bool {
        let job_market = self.job_market.lock();
        job_market.jobs.is_empty() && job_market.wait_count == self.thread_count
            || self.discoveries.len() == self.model.properties().len()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;
    use crate::test_util::linear_equation_solver::*;

    #[test]
    fn visits_states_in_dfs_order() {
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        LinearEquation { a: 2, b: 10, c: 14 }.checker()
            .visitor(recorder)
            .spawn_dfs().join();
        assert_eq!(
            accessor(),
            vec![
                (0,  0), (0,  1), (0,  2), (0,  3), (0,  4), (0,  5), (0,  6),
                (0,  7), (0,  8), (0,  9), (0, 10), (0, 11), (0, 12), (0, 13),
                (0, 14), (0, 15), (0, 16), (0, 17), (0, 18), (0, 19), (0, 20),
                (0, 21), (0, 22), (0, 23), (0, 24), (0, 25), (0, 26), (0, 27),
            ]);
    }

    #[cfg(not(debug_assertions))] // too slow for debug build
    #[test]
    fn can_complete_by_enumerating_all_states() {
        let checker = LinearEquation { a: 2, b: 4, c: 7 }.checker().spawn_dfs().join();
        assert_eq!(checker.is_done(), true);
        checker.assert_no_discovery("solvable");
        assert_eq!(checker.unique_state_count(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let checker = LinearEquation { a: 2, b: 10, c: 14 }.checker().spawn_dfs().join();
        checker.assert_properties();
        assert_eq!(checker.unique_state_count(), 55);

        // DFS found this example...
        assert_eq!(
            checker.discovery("solvable").unwrap().into_actions(),
            vec![Guess::IncreaseY; 27]); // (2*0 + 10*27) % 256 == 14
        // ... but there are of course other solutions, such as the following.
        checker.assert_discovery("solvable", vec![
            Guess::IncreaseX,
            Guess::IncreaseY,
            Guess::IncreaseX,
        ]);
    }

    #[test]
    fn can_apply_symmetry_reduction() {
        use crate::actor::Id;
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        struct Sys;
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        struct SysState(Vec<ProcState>);
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        enum ProcState {
            // A process advances from `Loading` to `Running` to cycling between `Paused` and
            // `Running`. There is no way for a process to move from `Loading` to `Paused` or any
            // non-`Loading` state to `Loading`, but a previous implementation of symmetry
            // reduction would mistakenly collect a path with an invalid step because it would
            // enqueue the representative of a state rather than the original state.
            //
            // Here is an example of the steps that would manifest that bug:
            //
            // 1. System starts: `[Loading, Loading]`
            // 2. Second process advances: `[Loading, Running]`
            // 3. Second process advances: `[Loading, Paused]`
            //
            // But in the third state above, the representative function swaps the process order
            // because `Paused < Loading`, so the collected path becomes:
            //
            // 1. `[Loading, Loading]`
            // 2. `[Loading, Running]`
            // 3. `[Paused, Loading]`
            //
            // Both `Loading -> Loading -> Paused` (process 0) and `Loading -> Running -> Loading`
            // (process 1) are invalid.
            Paused,
            Loading,
            Running,
        }
        impl Model for Sys {
            type Action = Id;
            type State = SysState;
            fn init_states(&self) -> Vec<SysState> {
                vec![SysState(vec![ProcState::Loading, ProcState::Loading])]
            }
            fn actions(&self, _: &Self::State, actions: &mut Vec<Self::Action>) {
                // Either process can run next.
                actions.push(Id::from(0));
                actions.push(Id::from(1));
            }
            fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
                let i = usize::from(action);
                let mut state = state.clone();
                match state.0[i]  {
                    ProcState::Loading => state.0[i] = ProcState::Running,
                    ProcState::Running => state.0[i] = ProcState::Paused,
                    ProcState::Paused => state.0[i] = ProcState::Running,
                }
                Some(state)
            }
            fn properties(&self) -> Vec<Property<Self>> {
                vec![
                    Property::<Self>::always(
                        "visit all states",
                        |_, _| true),
                    Property::<Self>::sometimes(
                        "a process pauses",
                        |_, s| s.0[0] == ProcState::Paused || s.0[1] == ProcState::Paused),
                ]
            }
        }
        impl Representative for SysState {
            fn representative(&self) -> Self {
                let plan = RewritePlan::from_values_to_sort(&self.0);
                SysState(plan.reindex(&self.0))
            }
        }
        impl Rewrite<Id> for ProcState {
            fn rewrite<S>(&self, _: &RewritePlan<Id, S>) -> Self {
                self.clone()
            }
        }

        // 9 states without symmetry reduction.
        let checker = Sys.checker().spawn_dfs().join();
        assert_eq!(checker.unique_state_count(), 9);
        let checker = Sys.checker().spawn_bfs().join();
        assert_eq!(checker.unique_state_count(), 9);

        // 6 states with symmetry reduction. `PathRecorder` panics upon encountering an invalid
        // path, and this was observed in a previous implementation with a bug.
        let (visitor, _) = PathRecorder::new_with_accessor();
        let checker = Sys.checker().symmetry().visitor(visitor).spawn_dfs().join();
        assert_eq!(checker.unique_state_count(), 6);
    }
}
