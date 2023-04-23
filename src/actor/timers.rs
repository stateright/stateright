use crate::{util::HashableHashSet, Rewrite, RewritePlan};
use std::hash::Hash;

use super::Id;

/// A collection of timers that have been set for a given actor.
#[derive(Clone, Debug, Hash, PartialEq, Eq, serde::Serialize)]
pub struct Timers<T: Hash + Eq>(HashableHashSet<T>);

impl<T: Hash + Eq> Default for Timers<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Timers<T>
where
    T: Hash + Eq,
{
    /// Create a new timer set.
    pub fn new() -> Self {
        Self(HashableHashSet::new())
    }

    /// Set a timer.
    pub fn set(&mut self, timer: T) -> bool {
        self.0.insert(timer)
    }

    /// Cancel a timer.
    pub fn cancel(&mut self, timer: &T) -> bool {
        self.0.remove(timer)
    }

    /// Cancels all timers.
    pub fn cancel_all(&mut self) {
        self.0.clear();
    }

    /// Iterate through the currently set timers.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T> Rewrite<Id> for Timers<T>
where
    T: Eq + Hash + Clone,
{
    fn rewrite<S>(&self, _plan: &RewritePlan<Id, S>) -> Self {
        self.clone()
    }
}
