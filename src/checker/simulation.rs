//! Private module for selective re-export.

use crate::checker::{Checker, Expectation, Path};
use crate::{fingerprint, CheckerBuilder, CheckerVisitor, Fingerprint, Model, Property};
use dashmap::DashMap;
use rand::rngs::SmallRng;
use rand::Rng;
use rand::SeedableRng;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use super::EventuallyBits;

// While this file is currently quite similar to dfs.rs, a refactoring to lift shared
// behavior is being postponed until DPOR is implemented.

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
    pub(crate) fn spawn(options: CheckerBuilder<M>) -> Self {
        let model = Arc::new(options.model);
        let symmetry = options.symmetry;
        let target_state_count = options.target_state_count;
        let target_max_depth = options.target_max_depth;
        let visitor = Arc::new(options.visitor);
        let property_count = model.properties().len();

        let state_count = Arc::new(AtomicUsize::new(0));
        let max_depth = Arc::new(AtomicUsize::new(0));
        let discoveries = Arc::new(DashMap::default());
        let mut handles = Vec::new();

        for t in 0..options.thread_count {
            let model = Arc::clone(&model);
            let visitor = Arc::clone(&visitor);
            let state_count = Arc::clone(&state_count);
            let max_depth = Arc::clone(&max_depth);
            let discoveries = Arc::clone(&discoveries);
            // create a per-thread rng to get them searching different parts of the space.
            let mut rng = SmallRng::from_entropy();
            handles.push(
                std::thread::Builder::new()
                    .name(format!("checker-{}", t))
                    .spawn(move || {
                        log::debug!("{}: Thread started.", t);
                        loop {
                            Self::check_trace_from_initial(
                                &model,
                                &mut rng,
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
                            if discoveries.len() == property_count {
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
                        }
                    })
                    .expect("Failed to spawn a thread"),
            );
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
    fn check_trace_from_initial(
        model: &M,
        rng: &mut SmallRng,
        state_count: &AtomicUsize,
        discoveries: &DashMap<&'static str, Vec<Fingerprint>>,
        visitor: &Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
        target_max_depth: Option<NonZeroUsize>,
        global_max_depth: &AtomicUsize,
        symmetry: Option<fn(&M::State) -> M::State>,
    ) {
        let properties = model.properties();

        let mut state = {
            let mut initial_states = model.init_states();
            let index = rng.gen_range(0, initial_states.len());
            initial_states.swap_remove(index)
        };

        let mut current_max_depth = global_max_depth.load(Ordering::Relaxed);
        let mut actions = Vec::new();
        let mut depth = 0;
        let mut fingerprint_path = Vec::new();
        let mut generated = HashSet::new();
        let mut choose_action = |actions: &[M::Action]| rng.gen_range(0, actions.len());
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
        let mut is_terminal = true;
        loop {
            // check max depth for this run
            // pick a state from the current states
            // generate the actions from this
            // generate the next states
            // update the current states
            if depth > current_max_depth {
                let _ = global_max_depth.compare_exchange(
                    current_max_depth,
                    depth,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                current_max_depth = depth;
            }
            if let Some(target_max_depth) = target_max_depth {
                if depth >= target_max_depth.get() {
                    log::trace!("Skipping exploring more states as past max depth {}", depth);
                    // return not break here as we do not know if this is terminal.
                    return;
                }
            }

            fingerprint_path.push(fingerprint(&state));
            depth += 1;

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
                break;
            }

            // generate the possible next actions
            model.actions(&state, &mut actions);

            // now pick one
            let index = choose_action(&actions);
            let action = actions.swap_remove(index);
            // now clear the actions for the next round
            actions.clear();

            state = match model.next_state(&state, action) {
                None => break,
                Some(next_state) => next_state,
            };

            // Skip if outside boundary.
            if !model.within_boundary(&state) {
                break;
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
            if let Some(representative) = symmetry {
                let representative_fingerprint = fingerprint(&representative(&state));
                if !generated.insert(representative_fingerprint) {
                    is_terminal = false;
                    break;
                }
                // IMPORTANT: continue the path with the pre-canonicalized state/fingerprint to
                // avoid jumping to another part of the state space for which there may not be
                // a path extension from the previously collected path.
            } else {
                let next_fingerprint = fingerprint(&state);
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
                    break;
                }
            };
        }
        if is_terminal {
            for (i, property) in properties.iter().enumerate() {
                if ebits.contains(i) {
                    // Races other threads, but that's fine.
                    discoveries.insert(property.name, fingerprint_path.clone());
                }
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

    #[cfg(not(debug_assertions))] // too slow for debug build
    #[test]
    fn can_complete_by_enumerating_all_states() {
        let checker = LinearEquation { a: 2, b: 4, c: 7 }
            .checker()
            .spawn_simulation()
            .join();
        assert_eq!(checker.is_done(), true);
        checker.assert_no_discovery("solvable");
        assert_eq!(checker.unique_state_count(), 256 * 256);
    }

    #[test]
    fn can_complete_by_eliminating_properties() {
        let checker = LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_simulation()
            .join();
        checker.assert_properties();

        checker.assert_discovery(
            "solvable",
            vec![Guess::IncreaseX, Guess::IncreaseY, Guess::IncreaseX],
        );
    }
}
