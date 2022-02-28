//! Private module for selective re-export.

// This can be made more efficient by introducing a `Network` trait once
// https://github.com/rust-lang/rust/issues/44265 stabilizes, enabling a
// `Network::Iterator<'a>` type constructor.
//
// ```
// trait Network<Msg> {
//     type Iterator<'a> ...
//     ...
// }
// ```

use crate::{Rewrite, RewritePlan};
use crate::actor::Id;
use crate::util::HashableHashSet;
use std::collections::{BTreeMap, VecDeque};
use std::hash::Hash;

/// Indicates the source and destination for a message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(serde::Serialize)]
pub struct Envelope<Msg> { pub src: Id, pub dst: Id, pub msg: Msg }

impl<Msg> Envelope<&Msg> {
    /// Converts an [`Envelope`] with a message reference to one that owns the message.
    pub fn to_cloned_msg(&self) -> Envelope<Msg>
    where Msg: Clone,
    {
        Envelope {
            src: self.src,
            dst: self.dst,
            msg: self.msg.clone(),
        }
    }
}

/// Represents a network of messages.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(serde::Serialize)]
pub enum Network<Msg>
where Msg: Eq + Hash
{
    /// Indicates that messages have no ordering and race one another.
    Unordered(HashableHashSet<Envelope<Msg>>),

    /// Indicates that directed message flows between pairs of actors are ordered. Does not
    /// indicate any ordering across different flows. Each direction for a pair of actors is a
    /// different flow.
    ///
    /// # See Also
    ///
    /// The [`ordered_reliable_link`] module partially implements this contract as long as actors do
    /// not restart. A later version of the module and checker will account for actor restarts.
    ///
    /// [`ordered_reliable_link`]: crate::actor::ordered_reliable_link
    Ordered(BTreeMap<(Id, Id), VecDeque<Msg>>),
}

impl<Msg> Network<Msg>
where Msg: Eq + Hash,
{
    /// Indicates that directed message flows between pairs of actors are ordered. Does not
    /// indicate any ordering across different flows. Each direction for a pair of actors is a
    /// different flow.
    ///
    /// # See Also
    ///
    /// The [`ordered_reliable_link`] module partially implements this contract as long as actors do
    /// not restart. A later version of the module and checker will account for actor restarts.
    ///
    /// [`ordered_reliable_link`]: crate::actor::ordered_reliable_link
    pub fn new_ordered(envelopes: impl IntoIterator<Item = Envelope<Msg>>) -> Self {
        let mut this = Self::Ordered(BTreeMap::new());
        for env in envelopes {
            this.send(env);
        }
        this
    }

    /// Indicates that messages have no ordering and race one another.
    pub fn new_unordered(envelopes: impl IntoIterator<Item = Envelope<Msg>>) -> Self {
        let mut this = Self::Unordered(
            HashableHashSet::with_hasher(
                crate::stable::build_hasher()));
        for env in envelopes {
            this.send(env);
        }
        this
    }

    /// Returns an iterator over all envelopes in the network.
    pub fn iter(&self) -> NetworkIter<Msg> {
        match self {
            Network::Unordered(set) => {
                NetworkIter::Unordered(set.iter())
            }
            Network::Ordered(map) => {
                NetworkIter::Ordered(
                    Box::new(
                        map.iter().flat_map(|(&(src, dst), messages)| {
                            messages.iter().map(move |msg| Envelope { src, dst, msg })
                        })))
            }
        }
    }

    /// Returns the number of messages in the network.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        match self {
            Network::Unordered(set) => set.len(),
            Network::Ordered(map) => map.values().map(VecDeque::len).sum(),
        }
    }

    /// Sends a message.
    pub(crate) fn send(&mut self, envelope: Envelope<Msg>) {
        match self {
            Network::Unordered(set) => {
                set.insert(envelope);
            }
            Network::Ordered(map) => {
                map.entry((envelope.src, envelope.dst))
                    .or_insert_with(|| VecDeque::with_capacity(1))
                    .push_back(envelope.msg);
            }
        }
    }

    /// Removes a message.
    pub(crate) fn remove(&mut self, envelope: &Envelope<Msg>)
    where Msg: PartialEq,
    {
        match self {
            Network::Unordered(set) => {
                set.remove(envelope);
            }
            Network::Ordered(map) => {
                // Find the flow, then find the message in the flow, and finally remove the message
                // from the flow. Flows must be non-empty (to ensure removing a message is the
                // inverse of adding it), so also canonicalize by deleting the entire flow if it
                // would be empty after removing the message.
                use std::collections::btree_map::Entry;
                let flow_entry = match map.entry((envelope.src, envelope.dst)) {
                    Entry::Vacant(_) => panic!("flow not found. src={:?}, dst={:?}", envelope.src, envelope.dst),
                    Entry::Occupied(flow) => flow,
                };
                let i = flow_entry.get().iter().position(|x| x == &envelope.msg).expect("message not found");
                if flow_entry.get().len() > 1 {
                    flow_entry.into_mut().remove(i);
                } else {
                    flow_entry.remove();
                }
            }
        }
    }
}

impl<Msg> Rewrite<Id> for Network<Msg>
where Msg: Eq + Hash + Rewrite<Id>,
{
    fn rewrite<S>(&self, plan: &RewritePlan<Id,S>) -> Self {
        match self {
            Network::Unordered(set) => Network::Unordered(set.rewrite(plan)),
            Network::Ordered(map) => Network::Ordered(map.rewrite(plan)),
        }
    }
}

pub enum NetworkIter<'a, Msg> {
    Unordered(std::collections::hash_set::Iter<'a, Envelope<Msg>>),
    Ordered(Box<dyn Iterator<Item = Envelope<&'a Msg>> + 'a>),
}

impl<'a, Msg> Iterator for NetworkIter<'a, Msg> {
    type Item = Envelope<&'a Msg>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            NetworkIter::Unordered(it) => {
                it.next().map(|env| Envelope {
                    src: env.src,
                    dst: env.dst,
                    msg: &env.msg,
                })
            },
            NetworkIter::Ordered(it) => it.next(),
        }
    }
}

