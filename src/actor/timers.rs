use crate::{util::HashableHashSet, Rewrite, RewritePlan};
use std::hash::Hash;

use super::Id;

#[derive(Clone, Debug, Hash, PartialEq, Eq, serde::Serialize)]
pub struct Timers<T: Hash + Eq>(HashableHashSet<T>);

impl<T> Timers<T>
where
    T: Hash + Eq,
{
    pub fn new() -> Self {
        Self(HashableHashSet::new())
    }

    pub fn insert(&mut self, timer: T) -> bool {
        self.0.insert(timer)
    }

    pub fn remove(&mut self, timer: &T) -> bool {
        self.0.remove(timer)
    }

    pub fn iter(&self) -> impl Iterator<Item=&T> {
        self.0.iter()
    }
}

impl<T> Rewrite<Id> for Timers<T>
where
    T: Eq + Hash + Clone ,
{
    fn rewrite<S>(&self, _plan: &RewritePlan<Id, S>) -> Self {
        self.clone()
    }
}
