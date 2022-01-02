//! Private module for selective re-export.

use crate::util::DenseNatMap;

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
/// [`Rewrite`]: crate::Rewrite
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
    pub(crate) fn from_values_to_sort<'a, V: 'a>(to_sort: impl IntoIterator<Item=&'a V>) -> Self
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

    /// Rewrites an instance of the "rewritten" type `R`. Calls to [`Rewrite`] will recurse until
    /// they find an instance of type `R` (in which case they call this method) or an instance that
    /// cannot contain an `R` (in which case they typically call [`Clone`] or [`Copy`]).
    /// 
    /// [`Rewrite`]: crate::Rewrite
    #[inline(always)]
    pub fn rewrite(&self, rewritten: &R) -> R
    where usize: From<R>,
          R: Clone + From<usize>,
    {
        self.rewrite_mapping[usize::from(rewritten.clone())].clone().into()
    }
}
