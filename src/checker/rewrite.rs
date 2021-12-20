//! Private module for selective re-export.

use std::hash::Hash;
use crate::actor::{Envelope, Id, Network};

/// Implementations can rewrite a data structure based on a mapping from old values to new values.
/// This is used for symmetry reduction when a [`Model::State`] implements [`Representative`].
///
/// See [`Reindex`] to use a mapping to revise indices for a collection.
///
/// [`Model::State`]: crate::Model::State
/// [`Representative`]: crate::Representative
/// [`Reindex`]: crate::Reindex
pub trait Rewrite<T> {
    /// Generates a corresponding instance with values revised based on `mapping`.
    fn rewrite(&self, mapping: &T) -> Self;
}

impl<T> Rewrite<T> for () {
    fn rewrite(&self, _: &T) -> Self {
        ()
    }
}

impl Rewrite<Vec<Id>> for Vec<Id> {
    fn rewrite(&self, mapping: &Vec<Id>) -> Self {
        self.iter()
            .map(|id| mapping[usize::from(*id)])
            .collect()
    }
}

impl Rewrite<Vec<usize>> for Vec<usize> {
    fn rewrite(&self, mapping: &Vec<usize>) -> Self {
        self.iter()
            .map(|i| mapping[*i])
            .collect()
    }
}

impl<Msg> Rewrite<Vec<Id>> for Network<Msg>
    where Msg: Clone + Eq + Hash,
{
    fn rewrite(&self, mapping: &Vec<Id>) -> Self {
        self.iter().map(|e| {
            Envelope {
                src: mapping[usize::from(e.src)],
                dst: mapping[usize::from(e.dst)],
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
