//! Private module for selective re-export.

use crate::actor::Id;
use std::collections::BTreeMap;

/// Implementations can reindex a collection based on a mapping from old indices to new indices.
/// This is used for symmetry reduction when a [`Model::State`] implements [`Representative`].
///
/// See [`Rewrite`] to use a mapping to revise values for a data structure.
///
/// [`Model::State`]: crate::Model::State
/// [`Representative`]: crate::Representative
/// [`Rewrite`]: crate::Rewrite
pub trait Reindex<M> {
    /// Generates a corresponding instance with indices revised based on `mapping`.
    fn reindex(&self, mapping: &M) -> Self;
}

// for BTreeMap
impl<V> Reindex<Vec<usize>> for BTreeMap<usize, V>
where V: Clone,
{
    fn reindex(&self, mapping: &Vec<usize>) -> Self {
        self.iter()
            .map(|(k, v)| (mapping[*k], v.clone()))
            .collect()
    }
}
impl<V> Reindex<Vec<Id>> for BTreeMap<Id, V>
where V: Clone,
{
    fn reindex(&self, mapping: &Vec<Id>) -> Self {
        self.iter()
            .map(|(k, v)| (mapping[usize::from(*k)], v.clone()))
            .collect()
    }
}
#[cfg(test)]
mod test_reindexing_btreemap {
    use super::*;
    #[test]
    fn can_reindex_btreemap_with_vec_of_usize() {
        let original: BTreeMap<_, _> = vec![
            (0, 'A'),
            (1, 'B'),
            (2, 'C'),
        ].into_iter().collect();
        assert_eq!(
            original.reindex(&vec![2, 1, 0]), // swap first and last
            vec![
                (2, 'A'),
                (1, 'B'),
                (0, 'C'),
            ].into_iter().collect());
        assert_eq!(
            original.reindex(&vec![1, 2, 0]), // rotate left
            vec![
                (1, 'A'),
                (2, 'B'),
                (0, 'C'),
            ].into_iter().collect());
    }
    #[test]
    fn can_reindex_btreemap_with_vec_of_id() {
        let original: BTreeMap<Id, _> = vec![
            (0.into(), 'A'),
            (1.into(), 'B'),
            (2.into(), 'C'),
        ].into_iter().collect();
        assert_eq!(
            // TODO: Id::vec_from(...)
            original.reindex(&Id::vec_from(vec![2, 1, 0])), // swap first and last
            vec![
                (2.into(), 'A'),
                (1.into(), 'B'),
                (0.into(), 'C'),
            ].into_iter().collect());
        assert_eq!(
            original.reindex(&Id::vec_from(vec![1, 2, 0])), // rotate left
            vec![
                (1.into(), 'A'),
                (2.into(), 'B'),
                (0.into(), 'C'),
            ].into_iter().collect());
    }
}


// for Vec
impl<V> Reindex<Vec<usize>> for Vec<V>
where V: Clone,
{
    fn reindex(&self, mapping: &Vec<usize>) -> Self {
        mapping.iter()
            .map(|i| self[*i].clone())
            .collect()
    }
}
impl<V> Reindex<Vec<Id>> for Vec<V>
where V: Clone,
{
    fn reindex(&self, mapping: &Vec<Id>) -> Self {
        mapping.iter()
            .map(|id| self[usize::from(*id)].clone())
            .collect()
    }
}
#[cfg(test)]
mod test_reindexing_vec {
    use super::*;
    #[test]
    fn can_reindex_vec_with_vec_of_usize() {
        let original = vec!['A', 'B', 'C'];
        assert_eq!(
            original.reindex(&vec![2, 1, 0]), // swap first and last
            vec!['C', 'B', 'A']);
        assert_eq!(
            original.reindex(&vec![1, 2, 0]), // rotate left
            vec!['B', 'C', 'A']);
    }
    #[test]
    fn can_reindex_vec_with_vec_of_id() {
        let original = vec!['A', 'B', 'C'];
        assert_eq!(
            original.reindex(&Id::vec_from(vec![2, 1, 0])), // swap first and last
            vec!['C', 'B', 'A']);
        assert_eq!(
            original.reindex(&Id::vec_from(vec![1, 2, 0])), // rotate left
            vec!['B', 'C', 'A']);
    }
}

