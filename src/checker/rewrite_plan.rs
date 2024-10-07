//! Private module for selective re-export.

use crate::util::DenseNatMap;
use crate::Rewrite;
use std::fmt;
use std::iter::FromIterator;
use std::ops::Index;

/// A `RewritePlan<R>` is derived from a data structure instance and indicates how values of type
/// `R` (short for "rewritten") should be rewritten. When that plan is recursively applied via
/// [`Rewrite`], the resulting data structure instance will be behaviorally equivalent to the
/// original data structure under a symmetry equivalence relation, enabling symmetry reduction.
///
/// Typically the `RewritePlan` would be constructed by an implementation of [`Representative`] for
/// [`Model::State`].
///
/// [`Model::State`]: crate::Model::State
/// [`Representative`]: crate::Representative
pub struct RewritePlan<R, S> {
    s: S,
    f: fn(&R, &S) -> R,
}

impl<R, S> RewritePlan<R, S> {
    /// Applies the rewrite plan to a value of type R
    pub fn rewrite(&self, x: &R) -> R {
        (self.f)(x, &self.s)
    }

    /// Returns the state. Useful for debugging
    pub fn get_state(&self) -> &S {
        &self.s
    }

    /// Creates a new rewrite plan from a given state and function
    pub fn new(s: S, f: fn(&R, &S) -> R) -> Self {
        RewritePlan { s, f }
    }
}

impl<R, S> fmt::Debug for RewritePlan<R, S>
where
    S: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("RewritePlan")
            .field("S", &self.s)
            .finish_non_exhaustive()
    }
}

impl<R, V> From<DenseNatMap<R, V>> for RewritePlan<R, DenseNatMap<R, R>>
where
    R: From<usize> + Copy,
    usize: From<R>,
    V: Ord,
{
    fn from(s: DenseNatMap<R, V>) -> Self {
        Self::from_values_to_sort(s.values())
    }
}

impl<R, V> From<&DenseNatMap<R, V>> for RewritePlan<R, DenseNatMap<R, R>>
where
    R: From<usize> + Copy,
    usize: From<R>,
    V: Ord,
{
    fn from(s: &DenseNatMap<R, V>) -> Self {
        Self::from_values_to_sort(s.values())
    }
}

impl<R> RewritePlan<R, DenseNatMap<R, R>>
where
    R: From<usize> + Copy,
    usize: From<R>,
{
    /// Constructs a `RewritePlan` by sorting values in a specified iterator. Favor using the
    ///  [`RewritePlan::new`] constructor over this one as it provides additional type safety.
    pub fn from_values_to_sort<'a, V>(to_sort: impl IntoIterator<Item = &'a V>) -> Self
    where
        R: From<usize>,
        usize: From<R>,
        V: 'a + Ord,
    {
        // Example in comments
        // [B,C,A]
        let mut combined = to_sort.into_iter().enumerate().collect::<Vec<_>>();
        // [(0,B), (1,C), (2,A)]
        combined.sort_by_key(|(_, v)| *v);
        // [(2,A), (0,B), (1,C)]
        let mut combined: Vec<_> = combined.iter().enumerate().collect();
        // [(0,(2,A)), (1,(0,B)), (2,(1,C))]
        combined.sort_by_key(|(_, (i, _))| i);
        // [(1,(0,B)), (2,(1,C)), (0,(2,A))]
        let map: DenseNatMap<R, R> = combined
            .iter()
            .map(|(sid, (_, _))| (*sid).into())
            .collect::<Vec<R>>()
            .into();
        RewritePlan {
            s: map,
            f: (|&x, s| *s.get(x).unwrap()),
        }
    }

    /// Permutes the elements of a [`Vec`]-like collection whose indices correspond with the
    /// indices of the `Vec`-like that was used to construct this `RewritePlan`.
    pub fn reindex<C>(&self, indexed: &C) -> C
    where
        C: Index<usize>,
        C: FromIterator<C::Output>,
        C::Output: Rewrite<R> + Sized,
    {
        let mut inverse_map = self.s.iter().map(|(i, &v)| (v, i)).collect::<Vec<_>>();
        inverse_map.sort_by_key(|(k, _)| Into::<usize>::into(*k));

        inverse_map
            .iter()
            .map(|(_, i)| indexed[(*i).into()].rewrite(self))
            .collect::<C>()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::actor::Id;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    #[test]
    fn from_sort_sorts() {
        let original = vec!['B', 'D', 'C', 'A'];
        let plan = RewritePlan::<Id, _>::from_values_to_sort(&original);
        assert_eq!(plan.reindex(&original), vec!['A', 'B', 'C', 'D']);
        assert_eq!(plan.reindex(&vec![1, 3, 2, 0]), vec![0, 1, 2, 3]);
    }

    #[test]
    fn can_reindex() {
        use crate::actor::Id;
        let swap_first_and_last = RewritePlan::<Id, _>::from_values_to_sort(&vec![2, 1, 0]);
        let rotate_left = RewritePlan::<Id, _>::from_values_to_sort(&vec![2, 0, 1]);

        let original = vec!['A', 'B', 'C'];
        assert_eq!(swap_first_and_last.reindex(&original), vec!['C', 'B', 'A']);
        assert_eq!(rotate_left.reindex(&original), vec!['B', 'C', 'A']);

        let original: VecDeque<_> = vec!['A', 'B', 'C'].into_iter().collect();
        assert_eq!(
            swap_first_and_last.reindex(&original),
            vec!['C', 'B', 'A'].into_iter().collect::<VecDeque<_>>()
        );
        assert_eq!(
            rotate_left.reindex(&original),
            vec!['B', 'C', 'A'].into_iter().collect::<VecDeque<_>>()
        );
    }

    #[test]
    fn can_rewrite() {
        #[derive(Debug, PartialEq)]
        struct GlobalState {
            process_states: DenseNatMap<Id, char>,
            run_sequence: Vec<Id>,
            zombies1: BTreeSet<Id>,
            zombies2: BTreeMap<Id, bool>,
            zombies3: DenseNatMap<Id, bool>,
        }
        impl Rewrite<Id> for GlobalState {
            fn rewrite<S>(&self, plan: &RewritePlan<Id, S>) -> Self {
                Self {
                    process_states: self.process_states.rewrite(plan),
                    run_sequence: self.run_sequence.rewrite(plan),
                    zombies1: self.zombies1.rewrite(plan),
                    zombies2: self.zombies2.rewrite(plan),
                    zombies3: self.zombies3.rewrite(plan),
                }
            }
        }

        let gs = GlobalState {
            process_states: DenseNatMap::from_iter(['B', 'A', 'A', 'C']),
            run_sequence: Id::vec_from([2, 2, 2, 2, 3]).into_iter().collect(),
            zombies1: Id::vec_from([0, 2]).into_iter().collect(),
            zombies2: vec![(0.into(), true), (2.into(), true)]
                .into_iter()
                .collect(),
            zombies3: vec![true, false, true, false].into_iter().collect(),
        };
        let plan = (&gs.process_states).into();
        assert_eq!(
            gs.rewrite(&plan),
            GlobalState {
                process_states: DenseNatMap::from_iter(['A', 'A', 'B', 'C']),
                run_sequence: Id::vec_from([1, 1, 1, 1, 3]).into_iter().collect(),
                zombies1: Id::vec_from([1, 2]).into_iter().collect(),
                zombies2: vec![(1.into(), true), (2.into(), true)]
                    .into_iter()
                    .collect(),
                zombies3: vec![false, true, true, false].into_iter().collect(),
            }
        );
    }
}
