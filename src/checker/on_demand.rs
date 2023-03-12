//! Private module for selective re-export.

use crate::checker::{Checker, EventuallyBits, Expectation, Path};
use crate::{
    fingerprint, CheckerBuilder, CheckerVisitor, ControlFlow, Fingerprint, Model, Property,
};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use nohash_hasher::NoHashHasher;
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hash};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// While this file is currently quite similar to dfs.rs, a refactoring to lift shared
// behavior is being postponed until DPOR is implemented.

pub(crate) struct OnDemandChecker<M: Model> {
    // Immutable state.
    model: Arc<M>,
    thread_count: usize,
    handles: Vec<std::thread::JoinHandle<()>>,

    // Mutable state.
    job_market: Arc<Mutex<JobMarket<M::State>>>,
    state_count: Arc<AtomicUsize>,
    max_depth: Arc<AtomicUsize>,
    generated:
        Arc<DashMap<Fingerprint, Option<Fingerprint>, BuildHasherDefault<NoHashHasher<u64>>>>,
    discoveries: Arc<DashMap<&'static str, Fingerprint>>,
    control_flow: std::sync::mpsc::SyncSender<ControlFlow>,
}
struct JobMarket<State> {
    wait_count: usize,
    jobs: Vec<Job<State>>,
}
type Job<State> = VecDeque<(State, Fingerprint, EventuallyBits, NonZeroUsize)>;

impl<M> OnDemandChecker<M>
where
    M: Model + Send + Sync + 'static,
    M::State: Hash + Send + 'static,
{
    pub(crate) fn spawn(options: CheckerBuilder<M>) -> Self {
        let model = Arc::new(options.model);
        let target_state_count = options.target_state_count;
        let thread_count = options.thread_count;
        let visitor = Arc::new(options.visitor);
        let property_count = model.properties().len();

        let mut controlflow_channels = Vec::new();
        let (controlflow_to_check_sender, controlflow_to_check_receiver) =
            std::sync::mpsc::sync_channel(1);

        let init_states: Vec<_> = model
            .init_states()
            .into_iter()
            .filter(|s| model.within_boundary(s))
            .collect();
        let state_count = Arc::new(AtomicUsize::new(init_states.len()));
        let max_depth = Arc::new(AtomicUsize::new(0));
        let total_out_degree = Arc::new(AtomicUsize::new(0));
        let generated = Arc::new({
            let generated = DashMap::default();
            for s in &init_states {
                generated.insert(fingerprint(s), None);
            }
            generated
        });
        let ebits = {
            let mut ebits = EventuallyBits::new();
            for (i, p) in model.properties().iter().enumerate() {
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
                (s, fp, ebits.clone(), NonZeroUsize::new(1).unwrap())
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

            let (controlflow_sender, controlflow_receiver) = std::sync::mpsc::channel();
            controlflow_channels.push(controlflow_sender);

            handles.push(std::thread::Builder::new()
                .name(format!("checker-{}", t))
                .spawn(move || {
                log::debug!("{}: Thread started.", t);
                let mut pending = VecDeque::new();
                let mut targetted_pending = VecDeque::new();
                let mut wait_for_fingerprints = true;
                loop {
                    if pending.is_empty() {
                        pending = {
                            let mut job_market = job_market.lock();
                            match job_market.jobs.pop() {
                                None => {
                                    // Done if all are waiting.
                                    if job_market.wait_count == thread_count {
                                        log::debug!(
                                            "{}: No more work. Shutting down... gen={}",
                                            t,
                                            generated.len()
                                        );
                                        has_new_job.notify_all();
                                        return;
                                    }

                                    // Otherwise more work may become available.
                                    log::trace!(
                                        "{}: No jobs. Awaiting. blocked={}",
                                        t,
                                        job_market.wait_count
                                    );
                                    has_new_job.wait(&mut job_market);
                                    continue;
                                }
                                Some(job) => {
                                    job_market.wait_count -= 1;
                                    log::trace!(
                                        "{}: Job found. size={}, blocked={}",
                                        t,
                                        job.len(),
                                        job_market.wait_count
                                    );
                                    job
                                }
                            }
                        };
                        log::debug!(
                            "got new pending states: {:?}",
                            pending.iter().map(|(_, f, _, _)| f).collect::<Vec<_>>()
                        );
                    }

                    if wait_for_fingerprints {
                        // Step 0: wait for someone to ask us to do work
                        loop {
                            let control_flow = controlflow_receiver.recv();
                            if let Ok(control_flow) = control_flow {
                                match control_flow {
                                    ControlFlow::CheckFingerprint(fingerprint) => {
                                        log::debug!(
                                            "received fingerprint to check: {}, pending is {:?}",
                                            fingerprint,
                                            pending.iter().map(|(_, f, _, _)| f).collect::<Vec<_>>()
                                        );
                                        if pending.is_empty() {
                                            break;
                                        }
                                        if let Some(index) =
                                            pending.iter().position(|(_, f, _, _)| *f == fingerprint)
                                        {
                                            targetted_pending
                                                .push_back(pending.remove(index).unwrap());
                                            log::debug!("found matching fingerprint!");
                                            // found a matching fingerprint in our pending queue so we can
                                            // process this group
                                            break;
                                        }
                                    }
                                    ControlFlow::RunToCompletion => {
                                        log::debug!("{}: running to completion", t);
                                        wait_for_fingerprints = false;
                                        break;
                                    }
                                }
                            } else {
                                // no commands left so we can finish
                                return;
                            }
                        }
                        log::debug!("after waiting for fingerprints");
                    } else {
                        targetted_pending.append(&mut pending);
                    }

                    // Step 1: Do work.
                    Self::check_block(
                        &*model,
                        &*state_count,
                        &*generated,
                        &mut targetted_pending,
                        &*discoveries,
                        &*visitor,
                        1500,
                        &max_depth,
                        &total_out_degree,
                    );
                    pending.append(&mut targetted_pending);
                    if discoveries.len() == property_count {
                        log::debug!(
                            "{}: Discovery complete. Shutting down... gen={}",
                            t,
                            generated.len()
                        );
                        let mut job_market = job_market.lock();
                        job_market.wait_count += 1;
                        drop(job_market);
                        has_new_job.notify_all();
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
                        let mut job_market = job_market.lock();
                        let pieces =
                            1 + std::cmp::min(job_market.wait_count as usize, pending.len());
                        let size = pending.len() / pieces;
                        for _ in 1..pieces {
                            log::trace!(
                                "{}: Sharing work. blocked={}, size={}",
                                t,
                                job_market.wait_count,
                                size
                            );
                            job_market
                                .jobs
                                .push(pending.split_off(pending.len() - size));
                            has_new_job.notify_one();
                        }
                    } else if pending.is_empty() {
                        let mut job_market = job_market.lock();
                        job_market.wait_count += 1;
                    }
                }
            }).expect("Failed to spawn a thread"));
        }

        // spawn a thread to forward the fingerprints to check
        handles.push(std::thread::spawn(move || {
            for fingerprint in controlflow_to_check_receiver {
                for sender in &controlflow_channels {
                    let _ = sender.send(fingerprint);
                }
            }
        }));

        OnDemandChecker {
            model,
            thread_count,
            handles,
            job_market,
            state_count,
            max_depth,
            generated,
            discoveries,
            control_flow: controlflow_to_check_sender,
        }
    }

    fn check_block(
        model: &M,
        state_count: &AtomicUsize,
        generated: &DashMap<
            Fingerprint,
            Option<Fingerprint>,
            BuildHasherDefault<NoHashHasher<u64>>,
        >,
        pending: &mut Job<M::State>,
        discoveries: &DashMap<&'static str, Fingerprint>,
        visitor: &Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
        mut max_count: usize,
        global_max_depth: &AtomicUsize,
        total_out_degree: &AtomicUsize,
    ) {
        let properties = model.properties();

        let mut current_max_depth = global_max_depth.load(Ordering::Relaxed);
        let mut actions = Vec::new();
        let mut local_pending = pending.drain(..).collect::<Vec<_>>();
        loop {
            // Done if reached max count.
            if max_count == 0 {
                return;
            }
            max_count -= 1;

            // Done if none pending.
            let (state, state_fp, mut ebits, max_depth) = match local_pending.pop() {
                None => return,
                Some(pair) => pair,
            };

            if max_depth.get() > current_max_depth {
                let _ = global_max_depth.compare_exchange(current_max_depth, max_depth.get(), Ordering::Relaxed, Ordering::Relaxed);
                current_max_depth = max_depth.get();
            }

            if let Some(visitor) = visitor {
                visitor.visit(model, reconstruct_path(model, generated, state_fp));
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
                            discoveries.insert(property.name, state_fp);
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
                            discoveries.insert(property.name, state_fp);
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
            if !is_awaiting_discoveries {
                return;
            }

            // Otherwise enqueue newly generated states (with related metadata).
            let mut is_terminal = true;
            model.actions(&state, &mut actions);
            total_out_degree.fetch_add(actions.len(), Ordering::Relaxed);
            let next_states = actions.drain(..).flat_map(|a| {
                model
                    .next_state(&state, a)
                    .map(|s| (fingerprint(&state), s))
            });
            for (old_state_fp, next_state) in next_states {
                log::debug!(
                    "checker generated state transition: {} -> {}",
                    old_state_fp,
                    fingerprint(&next_state)
                );
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
                let next_fingerprint = fingerprint(&next_state);
                if let Entry::Vacant(next_entry) = generated.entry(next_fingerprint) {
                    next_entry.insert(Some(state_fp));
                } else {
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

                // Otherwise further checking is applicable.
                is_terminal = false;
                pending.push_front((next_state, next_fingerprint, ebits.clone(),
                        NonZeroUsize::new(max_depth.get() + 1).unwrap()));
            }
            if is_terminal {
                for (i, property) in properties.iter().enumerate() {
                    if ebits.contains(i) {
                        // Races other threads, but that's fine.
                        discoveries.insert(property.name, state_fp);
                    }
                }
            }
        }
    }
}

impl<M> Checker<M> for OnDemandChecker<M>
where
    M: Model,
    M::State: Hash,
{
    fn model(&self) -> &M {
        &self.model
    }

    fn check_fingerprint(&self, fingerprint: Fingerprint) {
        log::debug!("asking to check fingerprint {}", fingerprint);
        let _ = self
            .control_flow
            .send(ControlFlow::CheckFingerprint(fingerprint));
    }

    fn run_to_completion(&self) {
        let _ = self.control_flow.send(ControlFlow::RunToCompletion);
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

    fn out_degrees(&self) -> Vec<usize> {
        Vec::new()
    }

    fn in_degrees(&self) -> Vec<usize> {
        Vec::new()
    }

    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>> {
        self.discoveries
            .iter()
            .map(|mapref| {
                (
                    <&'static str>::clone(mapref.key()),
                    reconstruct_path(self.model(), &*self.generated, *mapref.value()),
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

fn reconstruct_path<M>(
    model: &M,
    generated: &DashMap<Fingerprint, Option<Fingerprint>, BuildHasherDefault<NoHashHasher<u64>>>,
    fp: Fingerprint,
) -> Path<M::State, M::Action>
where
    M: Model,
    M::State: Hash,
{
    // First build a stack of digests representing the path (with the init digest at top of
    // stack). Then unwind the stack of digests into a vector of states. The TLC model checker
    // uses a similar technique, which is documented in the paper "Model Checking TLA+
    // Specifications" by Yu, Manolios, and Lamport.

    let mut fingerprints = VecDeque::new();
    let mut next_fp = fp;
    while let Some(source) = generated.get(&next_fp) {
        match *source {
            Some(prev_fingerprint) => {
                fingerprints.push_front(next_fp);
                next_fp = prev_fingerprint;
            }
            None => {
                fingerprints.push_front(next_fp);
                break;
            }
        }
    }
    Path::from_fingerprints(model, fingerprints)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::*;
    use crate::*;

    #[test]
    fn visits_states_in_bfs_order() {
        let (recorder, accessor) = StateRecorder::new_with_accessor();
        LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .visitor(recorder)
            .spawn_bfs()
            .join();
        assert_eq!(
            accessor(),
            vec![
                (0, 0), // distance == 0
                (1, 0),
                (0, 1), // distance == 1
                (2, 0),
                (1, 1),
                (0, 2), // distance == 2
                (3, 0),
                (2, 1), // distance == 3
            ]
        );
    }

    #[test]
    fn can_complete_by_enumerating_all_states() {
        let checker = LinearEquation { a: 2, b: 4, c: 7 }
            .checker()
            .spawn_bfs()
            .join();
        assert_eq!(checker.is_done(), true);
        checker.assert_no_discovery("solvable");
        assert_eq!(checker.unique_state_count(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let checker = LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_bfs()
            .join();
        checker.assert_properties();
        assert_eq!(checker.unique_state_count(), 12);

        // bfs found this example...
        assert_eq!(
            checker.discovery("solvable").unwrap().into_actions(),
            // (2*2 + 10*1) % 256 == 14
            vec![Guess::IncreaseX, Guess::IncreaseX, Guess::IncreaseY,]
        );
        // ... but there are of course other solutions, such as the following.
        checker.assert_discovery(
            "solvable",
            // (2*0 + 10*27) % 256 == 14
            vec![Guess::IncreaseY; 27],
        );
    }
}
