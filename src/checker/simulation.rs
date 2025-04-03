//! Private module for selective re-export.

use crate::checker::{Checker, Expectation, Path};
use crate::{fingerprint, CheckerBuilder, CheckerVisitor, Fingerprint, Model, Property};
use dashmap::DashMap;
use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{sleep, JoinHandle};
use std::time::{Duration, SystemTime};

use super::EventuallyBits;

/// Choose transitions in the model.
///
/// Created once for each thread.
pub trait Chooser<M: Model>: Send + Clone + 'static {
    /// State of the chooser during a run.
    type State;

    /// Create a new chooser state from this seed for the run.
    fn new_state(&self, seed: u64) -> Self::State;

    /// Choose the initial state for this simulation run.
    fn choose_initial_state(&self, state: &mut Self::State, initial_states: &[M::State]) -> usize;

    /// Choose the next action to take from the current state.
    fn choose_action(
        &self,
        state: &mut Self::State,
        current_state: &M::State,
        actions: &[M::Action],
    ) -> usize;
}

/// A chooser that makes uniform choices.
#[derive(Clone)]
pub struct UniformChooser;

/// A chooser that makes uniform choices.
pub struct UniformChooserState {
    // FIXME: use a reproducible rng, one that will not change over versions.
    rng: StdRng,
}

impl<M> Chooser<M> for UniformChooser
where
    M: Model,
{
    type State = UniformChooserState;

    fn new_state(&self, seed: u64) -> Self::State {
        UniformChooserState {
            rng: StdRng::seed_from_u64(seed),
        }
    }

    fn choose_initial_state(
        &self,
        state: &mut Self::State,
        initial_states: &[<M as Model>::State],
    ) -> usize {
        state.rng.gen_range(0..initial_states.len())
    }

    fn choose_action(
        &self,
        state: &mut Self::State,
        _current_state: &<M as Model>::State,
        actions: &[<M as Model>::Action],
    ) -> usize {
        state.rng.gen_range(0..actions.len())
    }
}

pub(crate) struct SimulationChecker<M: Model> {
    // Immutable state.
    model: Arc<M>,
    handles: Vec<std::thread::JoinHandle<()>>,

    // Mutable state.
    state_count: Arc<AtomicUsize>,
    max_depth: Arc<AtomicUsize>,
    discoveries: Arc<DashMap<&'static str, Vec<Fingerprint>>>,
}

impl<M> SimulationChecker<M>
where
    M: Model + Send + Sync + 'static,
    M::State: Hash + Send + 'static,
{
    /// Create a simulation checker.
    ///
    /// `seed` is the seed for the random selection of actions between states.
    /// It is passed straight through to the first trace on the first thread to allow for
    /// reproducibility. For other threads and traces it is regenerated using a [`StdRng`].
    pub(crate) fn spawn<C: Chooser<M>>(options: CheckerBuilder<M>, seed: u64, chooser: C) -> Self {
        let model = Arc::new(options.model);
        let symmetry = options.symmetry;
        let target_state_count = options.target_state_count;
        let target_max_depth = options.target_max_depth;
        let visitor = Arc::new(options.visitor);
        let finish_when = Arc::new(options.finish_when);
        let properties = Arc::new(model.properties());

        let state_count = Arc::new(AtomicUsize::new(0));
        let max_depth = Arc::new(AtomicUsize::new(0));
        let discoveries = Arc::new(DashMap::default());
        let mut handles = Vec::new();

        let mut thread_seed = seed;

        let shutdown = Arc::new(AtomicBool::new(false));
        let sd = Arc::clone(&shutdown);
        if let Some(close_after) = options.timeout {
            let closing_time = SystemTime::now() + close_after;
            std::thread::Builder::new()
                .name("timeout".to_owned())
                .spawn(move || loop {
                    let now = SystemTime::now();
                    if closing_time < now {
                        log::debug!("Reached timeout, triggering shutdown");
                        sd.store(true, Ordering::Relaxed);
                    }
                    if sd.load(Ordering::Relaxed) {
                        break;
                    }
                    sleep(Duration::from_secs(1));
                })
                .unwrap();
        }

        for t in 0..options.thread_count {
            let model = Arc::clone(&model);
            let visitor = Arc::clone(&visitor);
            let finish_when = Arc::clone(&finish_when);
            let properties = Arc::clone(&properties);
            let state_count = Arc::clone(&state_count);
            let max_depth = Arc::clone(&max_depth);
            let discoveries = Arc::clone(&discoveries);
            let shutdown = Arc::clone(&shutdown);
            let chooser = chooser.clone();
            handles.push(
                std::thread::Builder::new()
                    .name(format!("checker-{}", t))
                    .spawn(move || {
                        let mut seed = thread_seed;
                        log::debug!("{}: Thread started with seed={}.", t, seed);
                        // FIXME: use a reproducible rng, one that will not change over versions.
                        let mut rng = StdRng::seed_from_u64(seed);
                        loop {
                            if shutdown.load(Ordering::Relaxed) {
                                log::debug!("{}: Got shutdown signal.", t);
                                break;
                            }

                            Self::check_trace_from_initial::<C>(
                                &model,
                                seed,
                                &chooser,
                                &state_count,
                                &discoveries,
                                &visitor,
                                target_max_depth,
                                &max_depth,
                                symmetry,
                            );

                            // Check whether we have found everything.
                            // All threads should reach this check and have the same result,
                            // leading them all to shut down together.
                            if finish_when.matches(
                                &discoveries.iter().map(|r| *r.key()).collect(),
                                &properties,
                            ) {
                                log::debug!("{}: Discovery complete. Shutting down...", t,);
                                return;
                            }
                            if let Some(target_state_count) = target_state_count {
                                if target_state_count.get() <= state_count.load(Ordering::Relaxed) {
                                    log::debug!(
                                        "{}: Reached target state count. Shutting down...",
                                        t,
                                    );
                                    return;
                                }
                            }

                            seed = rng.gen();
                            log::trace!("{}: Generated new thread seed={}", t, seed);
                        }
                    })
                    .expect("Failed to spawn a thread"),
            );
            thread_seed += 1;
        }
        SimulationChecker {
            model,
            handles,
            state_count,
            max_depth,
            discoveries,
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    fn check_trace_from_initial<C: Chooser<M>>(
        model: &M,
        seed: u64,
        chooser: &C,
        state_count: &AtomicUsize,
        discoveries: &DashMap<&'static str, Vec<Fingerprint>>,
        visitor: &Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
        target_max_depth: Option<NonZeroUsize>,
        global_max_depth: &AtomicUsize,
        symmetry: Option<fn(&M::State) -> M::State>,
    ) {
        let properties = model.properties();

        let mut chooser_state = chooser.new_state(seed);

        let mut state = {
            let mut initial_states = model.init_states();
            let index = chooser.choose_initial_state(&mut chooser_state, &initial_states);
            initial_states.swap_remove(index)
        };

        let mut current_max_depth = global_max_depth.load(Ordering::Relaxed);
        // The set of actions.
        let mut actions = Vec::new();
        // The path of the fingerprints.
        let mut fingerprint_path = Vec::new();
        // The fingerprints we've seen in this run, for preventing cycles.
        let mut generated = HashSet::new();
        let mut ebits = {
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
        'outer: loop {
            if fingerprint_path.len() > current_max_depth {
                let _ = global_max_depth.compare_exchange(
                    current_max_depth,
                    fingerprint_path.len(),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                current_max_depth = fingerprint_path.len();
            }
            if let Some(target_max_depth) = target_max_depth {
                if fingerprint_path.len() >= target_max_depth.get() {
                    log::trace!(
                        "Skipping exploring more states as past max depth {}",
                        fingerprint_path.len()
                    );
                    // return not break here as we do not know if this is terminal.
                    log::trace!("Reached max depth");
                    return;
                }
            }

            // Skip if outside boundary.
            if !model.within_boundary(&state) {
                log::trace!("Found state outside of boundary");
                break;
            }

            // add the current fingerprint to the path
            fingerprint_path.push(fingerprint(&state));
            // check that we haven't already seen this state
            let inserted = if let Some(representative) = symmetry {
                generated.insert(fingerprint(&representative(&state)))
            } else {
                generated.insert(fingerprint(&state))
            };
            if !inserted {
                // found a loop
                log::trace!("Found a loop");
                break;
            }

            state_count.fetch_add(1, Ordering::Relaxed);

            if let Some(visitor) = visitor {
                visitor.visit(
                    model,
                    Path::from_fingerprints(model, VecDeque::from(fingerprint_path.clone())),
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
                            discoveries.insert(property.name, fingerprint_path.clone());
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
                            discoveries.insert(property.name, fingerprint_path.clone());
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
                log::trace!("Found all discoveries");
                break;
            }

            // generate the possible next actions
            model.actions(&state, &mut actions);

            // generate the next state, repeatedly choosing an action until we get one or there are
            // no actions left to choose.
            loop {
                if actions.is_empty() {
                    // no actions to choose from
                    // break from the outer loop so that we still check eventually properties
                    log::trace!("No actions to choose from");
                    break 'outer;
                }

                // now pick one
                let index = chooser.choose_action(&mut chooser_state, &state, &actions);
                let action = actions.swap_remove(index);

                // take the chosen action
                match model.next_state(&state, action) {
                    None => {
                        // this action was ignored, try and choose another
                        log::trace!("No next state");
                    }
                    Some(next_state) => {
                        // now clear the actions for the next round
                        actions.clear();
                        state = next_state;
                        break;
                    }
                };
            }
        }
        // check the eventually properties
        for (i, property) in properties.iter().enumerate() {
            if ebits.contains(i) {
                // Races other threads, but that's fine.
                discoveries.insert(property.name, fingerprint_path.clone());
            }
        }
    }
}

impl<M> Checker<M> for SimulationChecker<M>
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
        // we do not keep track of all the states visited so can't provide an accurate unique state
        // count
        self.state_count.load(Ordering::Relaxed)
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
        self.handles.iter().all(|h| h.is_finished())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::*;

    #[test]
    fn can_complete_by_eliminating_properties() {
        let checker = LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_simulation(0, UniformChooser)
            .join();
        checker.assert_properties();

        checker.assert_discovery(
            "solvable",
            vec![Guess::IncreaseX, Guess::IncreaseY, Guess::IncreaseX],
        );
    }
}
