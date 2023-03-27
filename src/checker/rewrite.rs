//! Private module for selective re-export.

use crate::RewritePlan;
use crate::actor::{Envelope, Id};
use crate::util::{HashableHashSet, HashableHashMap};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::Hash;
use std::sync::Arc;

/// Implementations can rewrite their instances of the "rewritten" type `R` based on a specified
/// [`RewritePlan`].
///
/// This is used for symmetry reduction when a [`Model::State`] implements [`Representative`]. See
/// the latter docs for an example.
///
/// [`Model::State`]: crate::Model::State
/// [`Representative`]: crate::Representative
pub trait Rewrite<R> {
    /// Generates a corresponding instance with values revised based on a particular `RewritePlan`.
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self;
}

// Built-in scalar types have blanket "no-op" implementations that simply clone.
macro_rules! impl_noop_rewrite {
    ( $( $t:ty ),* ) => {
        $(
            impl<R> crate::Rewrite<R> for $t {
                #[inline(always)]
                fn rewrite<S>(&self, _: &RewritePlan<R,S>) -> Self { self.clone() }
            }
        )*
    }
}
impl_noop_rewrite![
    // Primitive scalar types
    (),
    bool,
    char,
    f32, f64,
    i8, i16, i32, i64, isize,
    &'static str,
    u8, u16, u32, u64, usize,

    // Other non-container types
    String
];

// Built-in container types delegate to their components.
impl<R, T0, T1> Rewrite<R> for (T0, T1)
where T0: Rewrite<R>,
      T1: Rewrite<R>,
{
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        (self.0.rewrite(plan), self.1.rewrite(plan))
    }
}
impl<R, T> Rewrite<R> for Arc<T>
where T: Rewrite<R>,
{
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        Arc::new((**self).rewrite(plan))
    }
}
impl<R, K, V> Rewrite<R> for BTreeMap<K, V>
where K: Ord + Rewrite<R>,
      V: Rewrite<R>,
{
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter()
            .map(|(k, v)| (k.rewrite(plan), v.rewrite(plan)))
            .collect()
    }
}
impl<R, V> Rewrite<R> for BTreeSet<V> where V: Ord + Rewrite<R> {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter().map(|x| x.rewrite(plan)).collect()
    }
}
impl<R, V> Rewrite<R> for HashableHashSet<V> where V: Eq + Hash + Rewrite<R> {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter().map(|x| x.rewrite(plan)).collect()
    }
}
impl<R, K, V> Rewrite<R> for HashableHashMap<K,V>
where V: Eq + Hash + Rewrite<R>,
      K: Eq + Hash + Rewrite<R>,
      {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter().map(|(k,v)| (k.rewrite(plan), v.rewrite(plan)) ).collect()
    }
}
impl<R, T> Rewrite<R> for Option<T>
where T: Rewrite<R>,
{
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.as_ref().map(|v| v.rewrite(plan))
    }
}
impl<R, V> Rewrite<R> for Vec<V> where V: Rewrite<R> {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter().map(|x| x.rewrite(plan)).collect()
    }
}
impl<R, V> Rewrite<R> for VecDeque<V> where V: Rewrite<R> {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<R,S>) -> Self {
        self.iter().map(|x| x.rewrite(plan)).collect()
    }
}

// Implementations for some of Stateright's types follow.
impl Rewrite<Id> for Id {
    #[inline(always)]
    fn rewrite<S>(&self, plan: &RewritePlan<Id,S>) -> Self {
        plan.rewrite(self)
    }
}
impl<Msg> Rewrite<Id> for Envelope<Msg>
where Msg: Rewrite<Id>,
{
    fn rewrite<S>(&self, plan: &RewritePlan<Id,S>) -> Self {
        Envelope {
            src: self.src.rewrite(plan),
            dst: self.dst.rewrite(plan),
            msg: self.msg.rewrite(plan),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{Rewrite, RewritePlan};
    use crate::actor::{Envelope, Id, Network};

    #[test]
    fn can_rewrite_id_vec() {
        let original = Id::vec_from(vec![1, 2, 2]);
        assert_eq!(
            original.rewrite(&RewritePlan::<Id,_>::from_values_to_sort(&vec![2, 0, 1])),
            Id::vec_from(vec![0, 1, 1]));
        assert_eq!(
            original.rewrite(&RewritePlan::<Id,_>::from_values_to_sort(&vec![0, 2, 1])),
            Id::vec_from(vec![2, 1, 1]));
    }

    #[test]
    #[rustfmt::skip]
    fn can_rewrite_network() {
        let original: Network<_> = Network::new_unordered_duplicating([
            // Id(0) sends peers "Write(X)" and receives two acks.
            Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
            Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
            Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },
            Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },

            // Id(2) sends peers "Write(Y)" and receives one ack.
            Envelope { src: 2.into(), dst: 0.into(), msg: "Write(Y)" },
            Envelope { src: 2.into(), dst: 1.into(), msg: "Write(Y)" },
            Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(Y)" },
        ]);
        assert_eq!(
            original.rewrite(&RewritePlan::from_values_to_sort(&vec![2, 0, 1])),
            Network::new_unordered_duplicating([
                // Id(2) sends peers "Write(X)" and receives two acks.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Ack(Y)" },
            ]));
        assert_eq!(
            original.rewrite(&RewritePlan::from_values_to_sort(&vec![0, 2, 1])),
            Network::new_unordered_duplicating([
                // Id(0) sends peers "Write(X)" and receives two acks.
                Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Ack(Y)" },
            ]));
    }
}
