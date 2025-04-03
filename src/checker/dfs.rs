//! Private module for selective re-export.

use crate::checker::{Checker, EventuallyBits, Expectation, Path};
use crate::job_market::JobBroker;
use crate::{fingerprint, CheckerBuilder, CheckerVisitor, Fingerprint, Model, Property};
use dashmap::{DashMap, DashSet};
use nohash_hasher::NoHashHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hash};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::SystemTime;

// While this file is currently quite similar to bfs.rs, a refactoring to lift shared
// behavior is being postponed until DPOR is implemented.

pub(crate) struct DfsChecker<M: Model> {
    // Immutable state.
    model: Arc<M>,
    handles: Vec<std::thread::JoinHandle<()>>,

    // Mutable state.
    job_broker: JobBroker<Job<M::State>>,
    state_count: Arc<AtomicUsize>,
    max_depth: Arc<AtomicUsize>,
    generated: Arc<DashSet<Fingerprint, BuildHasherDefault<NoHashHasher<u64>>>>,
    discoveries: Arc<DashMap<&'static str, Vec<Fingerprint>>>,
}
type Job<State> = (State, Vec<Fingerprint>, EventuallyBits, NonZeroUsize);

impl<M> DfsChecker<M>
where
    M: Model + Send + Sync + 'static,
    M::State: Hash + Send + 'static,
{
    pub(crate) fn spawn(options: CheckerBuilder<M>) -> Self {
        let model = Arc::new(options.model);
        let symmetry = options.symmetry;
        let target_state_count = options.target_state_count;
        let target_max_depth = options.target_max_depth;
        let thread_count = options.thread_count;
        let visitor = Arc::new(options.visitor);
        let finish_when = Arc::new(options.finish_when);
        let properties = Arc::new(model.properties());

        let init_states: Vec<_> = model
            .init_states()
            .into_iter()
            .filter(|s| model.within_boundary(s))
            .collect();
        let state_count = Arc::new(AtomicUsize::new(init_states.len()));
        let max_depth = Arc::new(AtomicUsize::new(0));
        let generated = Arc::new({
            let generated = DashSet::default();
            for s in &init_states {
                if let Some(representative) = symmetry {
                    generated.insert(fingerprint(&representative(s)));
                } else {
                    generated.insert(fingerprint(s));
                }
            }
            generated
        });
        let ebits = {
            let mut ebits = EventuallyBits::new();
            for (i, p) in properties.iter().enumerate() {
                if let Property {
                    expectation: Expectation::Eventually,
                    ..
                } = p
                {
                    ebits.insert(i);
                }
            }
            ebits
        };
        let pending: VecDeque<_> = init_states
            .into_iter()
            .map(|s| {
                let fp = fingerprint(&s);
                (s, vec![fp], ebits.clone(), NonZeroUsize::new(1).unwrap())
            })
            .collect();
        let discoveries = Arc::new(DashMap::default());
        let mut handles = Vec::new();

        let close_at = options.timeout.map(|t| SystemTime::now() + t);
        let mut job_broker = JobBroker::new(thread_count, close_at);
        job_broker.push(pending);

        for t in 0..thread_count {
            let model = Arc::clone(&model);
            let visitor = Arc::clone(&visitor);
            let finish_when = Arc::clone(&finish_when);
            let properties = Arc::clone(&properties);
            let mut job_broker = job_broker.clone();
            let state_count = Arc::clone(&state_count);
            let max_depth = Arc::clone(&max_depth);
            let generated = Arc::clone(&generated);
            let discoveries = Arc::clone(&discoveries);
            handles.push(
                std::thread::Builder::new()
                    .name(format!("checker-{}", t))
                    .spawn(move || {
                        log::debug!("{}: Thread started.", t);
                        let mut pending = VecDeque::new();
                        loop {
                            // Step 1: Do work.
                            if pending.is_empty() {
                                pending = {
                                    let jobs = job_broker.pop();
                                    if jobs.is_empty() {
                                        log::debug!(
                                            "{}: No more work. Shutting down... gen={}",
                                            t,
                                            generated.len()
                                        );
                                        return;
                                    }
                                    log::trace!("{}: Job found. size={}", t, jobs.len());
                                    jobs
                                };
                            }
                            Self::check_block(
                                &model,
                                &state_count,
                                &generated,
                                &mut pending,
                                &discoveries,
                                &visitor,
                                1500,
                                target_max_depth,
                                &max_depth,
                                symmetry,
                            );
                            if finish_when.matches(
                                &discoveries.iter().map(|r| *r.key()).collect(),
                                &properties,
                            ) {
                                log::debug!(
                                    "{}: Discovery complete. Shutting down... gen={}",
                                    t,
                                    generated.len()
                                );
                                return;
                            }
                            if let Some(target_state_count) = target_state_count {
                                if target_state_count.get() <= state_count.load(Ordering::Relaxed) {
                                    log::debug!(
                                        "{}: Reached target state count. Shutting down... gen={}",
                                        t,
                                        generated.len()
                                    );
                                    return;
                                }
                            }

                            // Step 2: Share work.
                            if pending.len() > 1 && thread_count > 1 {
                                job_broker.split_and_push(&mut pending);
                            }
                        }
                    })
                    .expect("Failed to spawn a thread"),
            );
        }
        DfsChecker {
            model,
            handles,
            job_broker,
            state_count,
            max_depth,
            generated,
            discoveries,
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    fn check_block(
        model: &M,
        state_count: &AtomicUsize,
        generated: &DashSet<Fingerprint, BuildHasherDefault<NoHashHasher<u64>>>,
        pending: &mut VecDeque<Job<M::State>>,
        discoveries: &DashMap<&'static str, Vec<Fingerprint>>,
        visitor: &Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
        mut max_count: usize,
        target_max_depth: Option<NonZeroUsize>,
        global_max_depth: &AtomicUsize,
        symmetry: Option<fn(&M::State) -> M::State>,
    ) {
        let properties = model.properties();

        let mut current_max_depth = global_max_depth.load(Ordering::Relaxed);
        let mut actions = Vec::new();
        loop {
            // Done if reached max count.
            if max_count == 0 {
                return;
            }
            max_count -= 1;

            // Done if none pending.
            let (state, fingerprints, mut ebits, max_depth) = match pending.pop_back() {
                None => return,
                Some(pair) => pair,
            };

            if max_depth.get() > current_max_depth {
                let _ = global_max_depth.compare_exchange(
                    current_max_depth,
                    max_depth.get(),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                current_max_depth = max_depth.get();
            }

            if let Some(target_max_depth) = target_max_depth {
                if max_depth >= target_max_depth {
                    log::trace!("Skipping state as past max depth {}", max_depth);
                    continue;
                }
            }
            if let Some(visitor) = visitor {
                visitor.visit(
                    model,
                    Path::from_fingerprints(model, VecDeque::from(fingerprints.clone())),
                );
            }

            // Done if discoveries found for all properties.
            let mut is_awaiting_discoveries = false;
            for (i, property) in properties.iter().enumerate() {
                if discoveries.contains_key(property.name) {
                    continue;
                }
                match property {
                    Property {
                        expectation: Expectation::Always,
                        condition: always,
                        ..
                    } => {
                        if !always(model, &state) {
                            // Races other threads, but that's fine.
                            discoveries.insert(property.name, fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    }
                    Property {
                        expectation: Expectation::Sometimes,
                        condition: sometimes,
                        ..
                    } => {
                        if sometimes(model, &state) {
                            // Races other threads, but that's fine.
                            discoveries.insert(property.name, fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    }
                    Property {
                        expectation: Expectation::Eventually,
                        condition: eventually,
                        ..
                    } => {
                        // The checker early exits after finding discoveries for every property,
                        // and "eventually" property discoveries are only identified at terminal
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
            if !is_awaiting_discoveries {
                return;
            }

            // Otherwise enqueue newly generated states (with related metadata).
            let mut is_terminal = true;
            model.actions(&state, &mut actions);
            for action in actions.drain(..) {
                let next_state = match model.next_state(&state, action) {
                    None => continue,
                    Some(next_state) => next_state,
                };

                // Skip if outside boundary.
                if !model.within_boundary(&next_state) {
                    continue;
                }
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
                    if !generated.insert(representative_fingerprint) {
                        is_terminal = false;
                        continue;
                    }
                    // IMPORTANT: continue the path with the pre-canonicalized state/fingerprint to
                    // avoid jumping to another part of the state space for which there may not be
                    // a path extension from the previously collected path.
                    fingerprint(&next_state)
                } else {
                    let next_fingerprint = fingerprint(&next_state);
                    if !generated.insert(next_fingerprint) {
                        // FIXME: arriving at an already-known state may be a loop (in which case it
                        // could, in a fancier implementation, be considered a terminal state for
                        // purposes of eventually-property checking) but it might also be a join in
                        // a DAG, which makes it non-terminal. These cases can be disambiguated (at
                        // some cost), but for now we just _don't_ treat them as terminal, and tell
                        // users they need to explicitly ensure model path-acyclicality when they're
                        // using eventually properties (using a boundary or empty actions or
                        // whatever).
                        is_terminal = false;
                        continue;
                    }
                    next_fingerprint
                };

                // Otherwise further checking is applicable.
                is_terminal = false;
                let mut next_fingerprints = Vec::with_capacity(1 + fingerprints.len());
                for f in &fingerprints {
                    next_fingerprints.push(*f);
                }
                next_fingerprints.push(next_fingerprint);
                pending.push_back((
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
where
    M: Model,
    M::State: Hash,
{
    fn model(&self) -> &M {
        &self.model
    }

    fn state_count(&self) -> usize {
        self.state_count.load(Ordering::Relaxed)
    }

    fn unique_state_count(&self) -> usize {
        self.generated.len()
    }

    fn max_depth(&self) -> usize {
        self.max_depth.load(Ordering::Relaxed)
    }

    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>> {
        self.discoveries
            .iter()
            .map(|mapref| {
                (
                    <&'static str>::clone(mapref.key()),
                    Path::from_fingerprints(self.model(), VecDeque::from(mapref.value().clone())),
                )
            })
            .collect()
    }

    fn handles(&mut self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut self.handles)
    }

    fn is_done(&self) -> bool {
        self.job_broker.is_closed() || self.discoveries.len() == self.model.properties().len()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::*;
    use crate::*;

    #[test]
    fn visits_states_in_dfs_order() {
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .visitor(recorder)
            .spawn_dfs()
            .join();
        assert_eq!(
            accessor(),
            vec![
                (0, 0),
                (0, 1),
                (0, 2),
                (0, 3),
                (0, 4),
                (0, 5),
                (0, 6),
                (0, 7),
                (0, 8),
                (0, 9),
                (0, 10),
                (0, 11),
                (0, 12),
                (0, 13),
                (0, 14),
                (0, 15),
                (0, 16),
                (0, 17),
                (0, 18),
                (0, 19),
                (0, 20),
                (0, 21),
                (0, 22),
                (0, 23),
                (0, 24),
                (0, 25),
                (0, 26),
                (0, 27),
            ]
        );
    }

    #[cfg(not(debug_assertions))] // too slow for debug build
    #[test]
    fn can_complete_by_enumerating_all_states() {
        let checker = LinearEquation { a: 2, b: 4, c: 7 }
            .checker()
            .spawn_dfs()
            .join();
        assert_eq!(checker.is_done(), true);
        checker.assert_no_discovery("solvable");
        assert_eq!(checker.unique_state_count(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let checker = LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_dfs()
            .join();
        checker.assert_properties();
        assert_eq!(checker.unique_state_count(), 55);

        // DFS found this example...
        assert_eq!(
            checker.discovery("solvable").unwrap().into_actions(),
            vec![Guess::IncreaseY; 27]
        ); // (2*0 + 10*27) % 256 == 14
           // ... but there are of course other solutions, such as the following.
        checker.assert_discovery(
            "solvable",
            vec![Guess::IncreaseX, Guess::IncreaseY, Guess::IncreaseX],
        );
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
                match state.0[i] {
                    ProcState::Loading => state.0[i] = ProcState::Running,
                    ProcState::Running => state.0[i] = ProcState::Paused,
                    ProcState::Paused => state.0[i] = ProcState::Running,
                }
                Some(state)
            }
            fn properties(&self) -> Vec<Property<Self>> {
                vec![
                    Property::<Self>::always("visit all states", |_, _| true),
                    Property::<Self>::sometimes("a process pauses", |_, s| {
                        s.0[0] == ProcState::Paused || s.0[1] == ProcState::Paused
                    }),
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

    // test that the checker shuts down all threads properly after a checker thread encounters a
    // panic in the model execution.
    #[test]
    #[should_panic]
    fn handles_panics_gracefully() {
        crate::test_util::panicker::Panicker
            .checker()
            .threads(2)
            .spawn_dfs()
            .join();
    }
}
