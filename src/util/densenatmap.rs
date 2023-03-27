//! Private module for selective re-export.

use crate::{Rewrite, RewritePlan};
use std::iter::FromIterator;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

/// A map optimized for cases where each key corresponds with a unique entry in the range
/// `[0..self.len()]` and vice versa (each number in that range corresponds with a unique key in
/// the map). `DenseNatMap<K, V>` serves as a replacement for a similar [`Vec`]`<V>` pattern but
/// provides additional type safety to distinguish indices derived from `K1: `[`Into`]`<usize>` versus
/// some other `K2: Into<usize>`.
///
/// # Purpose
///
/// For example, if a [model's state] has no gaps in `FileId`s or `ProcessId`s, one approach is the following.
///
/// ```rust
/// # struct Metadata;
/// # struct RawBytes;
/// struct MyStruct {
///     file_metadata:    Vec<Metadata>, // indexed by `Into::<usize>::into(FileId)`
///     file_contents:    Vec<RawBytes>, // indexed by `Into::<usize>::into(FileId)`
///     process_metadata: Vec<Metadata>, // indexed by `Into::<usize>::into(ProcessId)`
///     process_memory:   Vec<RawBytes>, // indexed by `Into::<usize>::into(ProcessId)`
/// }
/// ```
///
/// Unfortunately the above fails to indicate to the compiler that the `file_*` fields have a
/// different indexing relationship than the `process_*` fields. `DenseNatMap` serves the same
/// purpose as `Vec`, but in contrast with the above example, indexing by a key of the wrong type
/// would be a type error.
///
/// ```rust
/// # struct FileId;
/// # struct ProcessId;
/// # struct Metadata;
/// # struct RawBytes;
/// use stateright::util::DenseNatMap;
/// struct MyStruct {
///     file_metadata:    DenseNatMap<FileId,    Metadata>,
///     file_contents:    DenseNatMap<FileId,    RawBytes>,
///     process_metadata: DenseNatMap<ProcessId, Metadata>,
///     process_memory:   DenseNatMap<ProcessId, RawBytes>,
/// }
/// ```
///
/// # Usage
///
/// Multiple mechanisms are available to construct a `DenseNatMap`. For example:
///
/// 1. Construct an empty map with [`DenseNatMap::new`], then [insert] the key-value pairs in order:
///    first the pair whose key corresponds with `0`, then the pair whose key corresponds with `1`,
///    and so on. Note that **inserting out of order will panic**.
///    ```rust
///    # use stateright::actor::Id;
///    # use stateright::util::DenseNatMap;
///    let mut m = DenseNatMap::new();
///    m.insert(Id::from(0), "first");
///    m.insert(Id::from(1), "second");
///    ```
/// 2. Or leverage [`Iterator::collect`].
///    ```rust
///    # use stateright::actor::Id;
///    # use stateright::util::DenseNatMap;
///    let mut m: DenseNatMap<Id, &'static str> = vec![
///        (Id::from(1), "second"),
///        (Id::from(0), "first"),
///    ].into_iter().collect();
///    ```
///
/// [insert]: DenseNatMap::insert
/// [model's state]: crate::Model::State
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DenseNatMap<K, V> {
    values: Vec<V>,
    _key: PhantomData<K>,
}

impl<K, V> DenseNatMap<K, V> {
    /// Constructs an empty `DenseNatMap`.
    pub fn new() -> Self {
        Self::from(Vec::new())
    }

    /// Accepts a key, and returns [`None`] if invalid, otherwise [`Some`]`(value)`.
    pub fn get(&self, key: K) -> Option<&V> where usize: From<K> {
        let index = usize::from(key);
        self.values.get(index)
    }

    /// Inserts a key-value pair and returns [`None`] if no value was previously associated,
    /// otherwise [`Some`]`(previous_value)`. Panics if neither overwriting a key nor inserting at
    /// the end.
    pub fn insert(&mut self, key: K, mut value: V) -> Option<V>
    where usize: From<K>,
          K: From<usize>,
    {
        let index = usize::from(key);
        if index > self.values.len() {
            panic!("Out of bounds. index={}, len={}", index, self.values.len());
        }
        if index == self.values.len() {
            self.values.push(value);
            return None;
        }
        std::mem::swap(&mut self.values[index], &mut value);
        Some(value)
    }

    /// Returns an iterator over pairs in the map whereby values are borrowed.
    ///
    /// See also [`DenseNatMap::values`].
    pub fn iter(&self) -> impl Iterator<Item=(K, &V)>
    where K: From<usize>,
    {
        self.values.iter()
            .enumerate()
            .map(|(i, v)| (K::from(i), v))
    }

    /// Returns the number of elements in the map.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize { self.values.len() }

    /// Returns an iterator over values in the map.
    ///
    /// See also [`DenseNatMap::iter`].
    pub fn values(&self) -> impl Iterator<Item=&V> {
        self.values.iter()
    }
}

impl<K, V> Default for DenseNatMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> From<Vec<V>> for DenseNatMap<K, V> {
    fn from(values: Vec<V>) -> Self {
        Self {
            values,
            _key: PhantomData,
        }
    }
}

impl<K, V> FromIterator<(K, V)> for DenseNatMap<K, V> where usize: From<K> {
    fn from_iter<T: IntoIterator<Item=(K, V)>>(iter: T) -> Self {
        Self {
            values: {
                let mut pairs: Vec<_> = iter.into_iter()
                    .map(|(k, v)| (usize::from(k), v))
                    .collect();
                pairs.sort_by_key(|(k, _)| *k);
                pairs.into_iter()
                    .enumerate()
                    .map(|(i_expected, (i, v))| {
                        if i != i_expected {
                            panic!("Invalid key at index. index={}, expected_index={}", i, i_expected);
                        }
                        v
                    })
                    .collect()
            },
            _key: PhantomData,
        }
    }
}

impl<K, V> FromIterator<V> for DenseNatMap<K, V> {
    fn from_iter<T: IntoIterator<Item=V>>(iter: T) -> Self {
        Self {
            values: iter.into_iter().collect(),
            _key: PhantomData,
        }
    }
}

impl<K, V> Index<K> for DenseNatMap<K, V>
where usize: From<K>,
{
    type Output = V;
    fn index(&self, key: K) -> &Self::Output {
        self.values.index(usize::from(key))
    }
}

impl<K, V> IndexMut<K> for DenseNatMap<K, V>
where usize: From<K>,
{
    fn index_mut(&mut self, key: K) -> &mut Self::Output {
        self.values.index_mut(usize::from(key))
    }
}

impl<K, V> IntoIterator for DenseNatMap<K, V> where K: From<usize> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter(
            self.values.into_iter().enumerate(),
            PhantomData)
    }
}

impl<R, K, V> Rewrite<R> for DenseNatMap<K, V>
where K: From<usize> + Rewrite<R>,
      V: Rewrite<R>,
      usize: From<K>,
{
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R, S>) -> Self {
        // FIXME: simply reindexing the keys (while still rewriting the values) would be more
        //        efficient, but we need to vary behavior based on the key type (i.e. for
        //        `DenseNatMap<R, _>`, `Rewrite<R>` would reindex, while `Rewrite<!R>` would not).
        self.iter()
            .map(|(k, v)| (k.rewrite(plan), v.rewrite(plan)))
            .collect()
    }
}

/// An iterator that moves out of a [`DenseNatMap`].
pub struct IntoIter<K, V>(
    std::iter::Enumerate<std::vec::IntoIter<V>>,
    PhantomData<K>);

impl<K, V> Iterator for IntoIter<K, V> where K: From<usize> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(i, v)| (K::from(i), v))
    }
}

#[cfg(test)]
mod test {
    use crate::actor::Id;
    use std::collections::{BTreeSet, BTreeMap};
    use super::*;

    #[test]
    pub fn can_construct_and_insert() {
        let mut m = DenseNatMap::new();
        m.insert(Id::from(0), "first");
        m.insert(Id::from(1), "second");
        assert_eq!(m.into_iter().collect::<BTreeSet<_>>(), vec![
            (Id::from(1), "second"), // out of order is fine here
            (Id::from(0), "first"),
        ].into_iter().collect());
    }

    #[test]
    #[should_panic(expected = "Out of bounds. index=1, len=0")]
    pub fn panics_on_out_of_order_insertion() {
        let mut m = DenseNatMap::new();
        m.insert(Id::from(1), "second");
    }

    #[test]
    #[rustfmt::skip]
    pub fn can_rewrite() {
        // This test relies on having a second rewritten type besides `Id`, which we call `Id2`.
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct Id2(usize);
        impl From<usize> for Id2 {
            fn from(input: usize) -> Self { Id2(input) }
        }
        impl From<Id2> for usize {
            fn from(input: Id2) -> Self { input.0 }
        }
        impl Rewrite<Id2> for Id2 {
            fn rewrite<S>(&self, plan: &RewritePlan<Id2, S>) -> Self {
                plan.rewrite(self)
            }
        }
        impl Rewrite<Id2> for Id {
            fn rewrite<S>(&self, _: &RewritePlan<Id2, S>) -> Self { *self }
        }
        impl Rewrite<Id> for Id2 {
            fn rewrite<S>(&self, _: &RewritePlan<Id, S>) -> Self { *self }
        }

        // Now we simulate the fields of a data structure that needs to be rewritten. Most "fields"
        // here leverage `Ids`, but the third field is indexed by `Id2`.
        let f1 = DenseNatMap::<Id, _>::from_iter(['B', 'C', 'A', 'D']);
        let f2 = DenseNatMap::<Id, _>::from_iter(['X', 'Y', 'W', 'Z']);
        let f3 = DenseNatMap::<Id2, _>::from_iter(['X', 'Y', 'W', 'Z']);
        let f4 = BTreeMap::from_iter([('!', Id::from(0))]);
        let f5 = BTreeMap::from_iter([(Id::from(0), '!')]);

        // Rewriting based on `Id` symmetry importantly does *not* reindex the third field.
        let plan = RewritePlan::from(&f1);
        assert_eq!(
            f1.rewrite(&plan),
            DenseNatMap::from_iter(['A', 'B', 'C', 'D']));
        assert_eq!(
            f2.rewrite(&plan),
            DenseNatMap::from_iter(['W', 'X', 'Y', 'Z']));
        assert_eq!(
            f3.rewrite(&plan),
            DenseNatMap::from_iter(['X', 'Y', 'W', 'Z']) // *not* reindexed
        );
        assert_eq!(
            f4.rewrite(&plan),
            BTreeMap::from_iter([('!', Id::from(1))]));
        assert_eq!(
            f5.rewrite(&plan),
            BTreeMap::from_iter([(Id::from(1), '!')]));

        // Rewriting based on `Id2` on the other hand impacts *only* the third field.
        let plan = RewritePlan::from(&f3);
        assert_eq!(
            f1.rewrite(&plan),
            DenseNatMap::from_iter(['B', 'C', 'A', 'D']) // *not* reindexed
        );
        assert_eq!(
            f2.rewrite(&plan),
            DenseNatMap::from_iter(['X', 'Y', 'W', 'Z']) // *not* reindexed
        );
        assert_eq!(
            f3.rewrite(&plan),
            DenseNatMap::from_iter(['W', 'X', 'Y', 'Z']));
        assert_eq!(
            f4.rewrite(&plan),
            BTreeMap::from_iter([('!', Id::from(0))]) // Id is *not* rewritten
        );
        assert_eq!(
            f5.rewrite(&plan),
            BTreeMap::from_iter([(Id::from(0), '!')]) // Id is *not* rewritten
        );
    }
}
