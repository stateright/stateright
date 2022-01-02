//! Private module for selective re-export.

use crate::Rewrite;
use crate::util::DenseNatMap;
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
#[derive(Debug, PartialEq)]
pub struct RewritePlan<R> {
    reindex_mapping: Vec<usize>,
    rewrite_mapping: Vec<R>,
}

impl<R> RewritePlan<R>
{
    /// Constructs a `RewritePlan` by sorting values in a specified [`DenseNatMap`].
    pub fn new<V>(input_field: &DenseNatMap<R, V>) -> Self
    where R: From<usize>,
          V: Ord,
    {
        Self::from_values_to_sort(input_field.values())
    }

    /// Constructs a `RewritePlan` by sorting values in a specified iterator. Favor using the
    ///  [`RewritePlan::new`] constructor over this one as it provides additional type safety.
    pub fn from_values_to_sort<'a, V: 'a>(to_sort: impl IntoIterator<Item=&'a V>) -> Self
    where R: From<usize>,
          V: Ord,
    {
        let mut combined = to_sort.into_iter()
            .enumerate()
            .map(|(i, s)| (s, i))
            .collect::<Vec<_>>();
        combined.sort();
        let reindex_mapping: Vec<usize> = combined.iter()
            .map(|(_, i)| *i)
            .collect();
        Self::from_reindex_mapping(reindex_mapping)
    }

    /// Constructs a `RewritePlan` from a specified remapping of indices. Favor using the
    ///  [`RewritePlan::new`] constructor over this one as it provides additional type safety.
    pub(self) fn from_reindex_mapping(reindex_mapping: Vec<usize>) -> Self
    where R: From<usize>,
    {
        let mut rewrite_mapping: Vec<_> = reindex_mapping.iter().enumerate()
            .map(|(dst, src)| (*src, dst))
            .collect();
        rewrite_mapping.sort();
        let rewrite_mapping: Vec<_> = rewrite_mapping.iter()
            .map(|(_, i)| R::from(*i))
            .collect();
        Self { reindex_mapping, rewrite_mapping }
    }

    /// Permutes the elements of a [`Vec`]-like collection whose indices correspond with the
    /// indices of the `Vec`-like that was used to construct this `RewritePlan`.
    pub fn reindex<C>(&self, indexed: &C) -> C
    where C: Index<usize>,
          C: FromIterator<C::Output>,
          C::Output: Rewrite<R> + Sized,
    {
        self.reindex_mapping.iter()
            .map(|i| indexed[*i].rewrite(&self))
            .collect()
    }

    /// Rewrites an instance of the "rewritten" type `R`. Calls to [`Rewrite`] will recurse until
    /// they find an instance of type `R` (in which case they call this method) or an instance that
    /// cannot contain an `R` (in which case they typically call [`Clone`] or [`Copy`]).
    /// 
    /// [`Rewrite`]: crate::Rewrite
    #[inline(always)]
    pub fn rewrite(&self, rewritten: R) -> R
    where usize: From<R>,
          R: Clone + From<usize>,
    {
        self.rewrite_mapping[usize::from(rewritten)].clone().into()
    }
}

#[cfg(test)]
mod test {
    use crate::actor::Id;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use super::*;

    #[test]
    fn can_reindex() {
        use crate::actor::Id;
        let swap_first_and_last = RewritePlan::<Id>::from_reindex_mapping(vec![2, 1, 0]);
        let rotate_left = RewritePlan::<Id>::from_reindex_mapping(vec![1, 2, 0]);

        let original = vec!['A', 'B', 'C'];
        assert_eq!(
            swap_first_and_last.reindex(&original),
            vec!['C', 'B', 'A']);
        assert_eq!(
            rotate_left.reindex(&original),
            vec!['B', 'C', 'A']);

        let original: VecDeque<_> = vec!['A', 'B', 'C'].into_iter().collect();
        assert_eq!(
            swap_first_and_last.reindex(&original),
            vec!['C', 'B', 'A'].into_iter().collect::<VecDeque<_>>());
        assert_eq!(
            rotate_left.reindex(&original),
            vec!['B', 'C', 'A'].into_iter().collect::<VecDeque<_>>());
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
            fn rewrite(&self, plan: &RewritePlan<Id>) -> Self {
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
            zombies2: vec![(0.into(), true), (2.into(), true)].into_iter().collect(),
            zombies3: vec![true, false, true, false].into_iter().collect(),
        };
        let plan = RewritePlan::new(&gs.process_states);
        assert_eq!(plan, RewritePlan {
            reindex_mapping: vec![1, 2, 0, 3],
            rewrite_mapping: vec![2.into(), 0.into(), 1.into(), 3.into()],
        });
        assert_eq!(gs.rewrite(&plan), GlobalState {
            process_states: DenseNatMap::from_iter(['A', 'A', 'B', 'C']),
            run_sequence: Id::vec_from([1, 1, 1, 1, 3]).into_iter().collect(),
            zombies1: Id::vec_from([1, 2]).into_iter().collect(),
            zombies2: vec![(1.into(), true), (2.into(), true)].into_iter().collect(),
            zombies3: vec![false, true, true, false].into_iter().collect(),
        });
    }
}
