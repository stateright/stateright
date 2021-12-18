//! Private module for selective re-export.

use std::hash::Hash;
use crate::actor::{Envelope, Id, Network};

/// Implementations can rewrite internal state based on a specified rewriting data structure `T`.
/// This is useful for symmetry reduction.
///
/// See also: [`Symmetric`].
///
/// [`Symmetric`]: crate::Symmetric
pub trait Rewrite<T> {
    /// Converts this instance into a corresponding one with the specified rewrites.
    fn rewrite(&self, rewrites: &T) -> Self;
}
impl<T> Rewrite<T> for () {
    fn rewrite(&self, _: &T) -> Self {
        ()
    }
}
impl Rewrite<Vec<Id>> for Vec<bool> {
    fn rewrite(&self, rewrites: &Vec<Id>) -> Self {
        rewrites.iter()
            .map(|id| self[usize::from(*id)])
            .collect()
    }
}
impl Rewrite<Vec<Id>> for Vec<Id> {
    fn rewrite(&self, rewrites: &Vec<Id>) -> Self {
        self.iter()
            .map(|id| rewrites[usize::from(*id)])
            .collect()
    }
}
impl<Msg> Rewrite<Vec<Id>> for Network<Msg>
    where Msg: Clone + Eq + Hash,
{
    fn rewrite(&self, rewrites: &Vec<Id>) -> Self {
        self.iter().map(|e| {
            Envelope {
                src: rewrites[usize::from(e.src)],
                dst: rewrites[usize::from(e.dst)],
                msg: e.msg.clone(),
            }
        }).collect()
    }
}

#[cfg(test)]
mod test {
    use crate::Rewrite;
    use crate::actor::{Envelope, Id, Network};

    #[test]
    fn can_rewrite_bool_vec() {
        let original = vec![true, false, false];
        assert_eq!(
            original.rewrite(&vec![2.into(), 0.into(), 1.into()]),
            vec![false, true, false]);
        assert_eq!(
            original.rewrite(&vec![0.into(), 2.into(), 1.into()]),
            vec![true, false, false]);
    }

    #[test]
    fn can_rewrite_id_vec() {
        let original: Vec<Id> = vec![1.into(), 2.into(), 2.into()];
        assert_eq!(
            original.rewrite(&vec![2.into(), 0.into(), 1.into()]),
            vec![0.into(), 1.into(), 1.into()]);
        assert_eq!(
            original.rewrite(&vec![0.into(), 2.into(), 1.into()]),
            vec![2.into(), 1.into(), 1.into()]);
    }

    #[test]
    fn can_rewrite_network() {
        let original: Network<_> = vec![
            // Id(0) sends peers "Write(X)" and receives two acks.
            Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
            Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
            Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },
            Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },

            // Id(2) sends peers "Write(Y)" and receives one ack.
            Envelope { src: 2.into(), dst: 0.into(), msg: "Write(Y)" },
            Envelope { src: 2.into(), dst: 1.into(), msg: "Write(Y)" },
            Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(Y)" },
        ].into_iter().collect();
        assert_eq!(
            original.rewrite(&vec![2.into(), 0.into(), 1.into()]),
            vec![
                // Id(2) sends peers "Write(X)" and receives two acks.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Ack(Y)" },
            ].into_iter().collect());
        assert_eq!(
            original.rewrite(&vec![0.into(), 2.into(), 1.into()]),
            vec![
                // Id(0) sends peers "Write(X)" and receives two acks.
                Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Ack(Y)" },
            ].into_iter().collect());
    }
}
