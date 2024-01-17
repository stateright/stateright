use std::collections::BTreeSet;

use crate::{Model, Property};

#[derive(Debug, Clone)]
pub enum HasDiscoveries {
    /// Wait for all discoveries to be made.
    All,
    /// Any, whatever is first.
    Any,
    /// Any, that counts as a failure.
    AnyFailures,
    /// Wait for all failures.
    AllFailures,
    /// All of a given set.
    AllOf(BTreeSet<&'static str>),
    /// Any of a given set.
    AnyOf(BTreeSet<&'static str>),
}

impl HasDiscoveries {
    pub fn matches<M: Model>(
        &self,
        discoveries: &BTreeSet<&'static str>,
        properties: &[Property<M>],
    ) -> bool {
        match self {
            HasDiscoveries::All => discoveries.len() == properties.len(),
            HasDiscoveries::Any => !discoveries.is_empty(),
            HasDiscoveries::AnyFailures => properties
                .iter()
                .filter(|prop| prop.expectation.discovery_is_failure())
                .any(|prop| discoveries.contains(prop.name)),
            HasDiscoveries::AllFailures => properties
                .iter()
                .filter(|prop| prop.expectation.discovery_is_failure())
                .all(|prop| discoveries.contains(prop.name)),
            HasDiscoveries::AllOf(props) => props.iter().all(|prop| discoveries.contains(prop)),
            HasDiscoveries::AnyOf(props) => props.iter().any(|prop| discoveries.contains(prop)),
        }
    }
}
