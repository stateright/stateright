//! Private module for selective re-export.

use crate::{Fingerprint, fingerprint, Model, Property};
use crate::checker::{Expectation, ModelChecker, Path};
use dashmap::DashSet;
use nohash_hasher::NoHashHasher;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash};

/// Generates every state reachable by a model, and verifies that all properties hold.
/// In contrast with [`BfsChecker`], this checker performs a depth first search,
/// which requires much less memory. Can be instantiated with [`DfsChecker::new`].
///
/// Temporary limitations:
///
/// - Has no support for multithreaded checking.
/// - Has no support for [`eventually`] properties.
///
/// See the implemented [`ModelChecker`] trait for helper methods.
///
/// [`BfsChecker`]: crate::BfsChecker
/// [`eventually`]: Property::eventually
pub struct DfsChecker<M: Model> {
    model: M,
    generated: DashSet<Fingerprint, BuildHasherDefault<NoHashHasher<u64>>>,
    pending: Vec<(M::State, Vec<Fingerprint>)>,
    discoveries: Vec<(Property<M>, Option<Vec<Fingerprint>>)>,
}

impl<M> ModelChecker<M> for DfsChecker<M>
where M: Model,
      M::State: Hash,
{
    fn model(&self) -> &M { &self.model }

    fn generated_count(&self) -> usize { self.generated.len() }

    fn pending_count(&self) -> usize { self.pending.len() }

    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>> {
        self.discoveries.iter()
            .filter_map(|(property, path)| {
                path.as_ref().map(|path| {
                    (
                        property.name,
                        Path::from_model_and_fingerprints(self.model(), path.clone())
                    )
                })
            })
            .collect()
    }

    fn check(&mut self, mut max_count: usize) -> &mut Self {
        let DfsChecker { discoveries, generated, model, pending } = self;
        let mut actions = Vec::new();
        loop {
            // Done if reached max count.
            if max_count == 0 { return self }
            max_count -= 1;

            // Done if none pending.
            let (state, fingerprints) = match pending.pop() {
                None => return self,
                Some(pair) => pair,
            };

            // Done if discoveries found for all properties.
            let mut is_awaiting_discoveries = false;
            for (property, discovery) in discoveries.iter_mut() {
                if discovery.is_some() { continue }
                match property {
                    Property { expectation: Expectation::Always, condition: always, .. } => {
                        if !always(model, &state) {
                            *discovery = Some(fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    },
                    Property { expectation: Expectation::Sometimes, condition: sometimes, .. } => {
                        if sometimes(model, &state) {
                            *discovery = Some(fingerprints.clone());
                        } else {
                            is_awaiting_discoveries = true;
                        }
                    },
                    Property { expectation: Expectation::Eventually, condition: _eventually, .. } => {
                        // FIXME: convert Graydon's implementation from BfsChecker.
                        unimplemented!();
                    }
                }
            }
            if !is_awaiting_discoveries { return self }

            // Otherwise enqueue newly generated states and fingerprint traces.
            model.actions(&state, &mut actions);
            let next_states = actions.drain(..).flat_map(|a| model.next_state(&state, a));
            for next_state in next_states {
                // Skip if outside boundary.
                if !model.within_boundary(&next_state) { continue }

                // Skip if already generated.
                let next_fingerprint = fingerprint(&next_state);
                if generated.contains(&next_fingerprint) { continue }
                generated.insert(next_fingerprint);

                // Otherwise checking is applicable.
                let mut next_fingerprints = Vec::with_capacity(1 + fingerprints.len());
                for f in &fingerprints { next_fingerprints.push(*f); }
                next_fingerprints.push(next_fingerprint);
                pending.push((next_state, next_fingerprints));
            }
        }
    }
}

impl<M> DfsChecker<M>
where M: Model,
      M::State: Hash,
{
    /// Instantiate a [`DfsChecker`].
    pub fn new(model: M) -> Self {
        let generated = DashSet::default();
        for s in model.init_states() { generated.insert(fingerprint(&s)); }

        Self {
            generated,
            discoveries: model.properties().into_iter()
                .map(|p| (p, None))
                .collect(),
            pending: model.init_states().into_iter()
                .map(|s| {
                    let fs = vec![fingerprint(&s)];
                    (s, fs)
                })
                .collect(),
            model,
        }
    }
}
