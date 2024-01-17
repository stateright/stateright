use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub enum HasDiscoveries {
    /// Wait for all discoveries to be made.
    All,
    /// Any, whatever is first.
    Any,
    /// All of a given set.
    AllOf(BTreeSet<&'static str>),
    /// Any of a given set.
    AnyOf(BTreeSet<&'static str>),
}

impl HasDiscoveries {
    pub fn matches(&self, discoveries: &BTreeSet<&'static str>, property_count: usize) -> bool {
        match self {
            HasDiscoveries::All => discoveries.len() == property_count,
            HasDiscoveries::Any => !discoveries.is_empty(),
            HasDiscoveries::AllOf(props) => props.iter().all(|prop| discoveries.contains(prop)),
            HasDiscoveries::AnyOf(props) => props.iter().any(|prop| discoveries.contains(prop)),
        }
    }
}
