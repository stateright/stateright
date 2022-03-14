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
use crate::util::{HashableHashMap, HashableHashSet};
use std::collections::{BTreeMap, VecDeque};
use std::collections::btree_map::Entry;
use std::collections::{btree_map, hash_set, hash_map};
use std::hash::Hash;

/// Indicates the source and destination for a message.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
    /// Indicates that messages have no ordering (racing one another), and can be redelivered.
    UnorderedDuplicating(HashableHashSet<Envelope<Msg>>),

    /// Indicates that messages have no ordering (racing one another), and will not be redelivered.
    UnorderedNonDuplicating(HashableHashMap<Envelope<Msg>, usize>),

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

    /// Indicates that messages have no ordering (racing one another), and can be redelivered.
    ///
    /// See also: [`Self::new_unordered_nonduplicating`]
    pub fn new_unordered_duplicating(envelopes: impl IntoIterator<Item = Envelope<Msg>>) -> Self {
        let mut this = Self::UnorderedDuplicating(
            HashableHashSet::with_hasher(
                crate::stable::build_hasher()));
        for env in envelopes {
            this.send(env);
        }
        this
    }

    /// Indicates that messages have no ordering (racing one another), and will not be redelivered.
    ///
    /// See also: [`Self::new_unordered_duplicating`]
    pub fn new_unordered_nonduplicating(envelopes: impl IntoIterator<Item = Envelope<Msg>>) -> Self {
        let mut this = Self::UnorderedNonDuplicating(
            HashableHashMap::with_hasher(
                crate::stable::build_hasher()));
        for env in envelopes {
            this.send(env);
        }
        this
    }

    /// Returns an iterator over all envelopes in the network.
    pub fn iter_all(&self) -> NetworkIter<Msg> {
        match self {
            Network::UnorderedDuplicating(set) => {
                NetworkIter::UnorderedDuplicating(set.iter())
            }
            Network::UnorderedNonDuplicating(multiset) => {
                NetworkIter::UnorderedNonDuplicating(None, multiset.iter())
            }
            Network::Ordered(map) => {
                NetworkIter::Ordered(None, map.iter())
            }
        }
    }

    /// Returns an iterator over all distinct deliverable envelopes in the network.
    pub fn iter_deliverable(&self) -> NetworkDeliverableIter<Msg> {
        match self {
            Network::UnorderedDuplicating(set) => {
                NetworkDeliverableIter::UnorderedDuplicating(set.iter())
            }
            Network::UnorderedNonDuplicating(multiset) => {
                NetworkDeliverableIter::UnorderedNonDuplicating(multiset.keys())
            }
            Network::Ordered(map) => {
                NetworkDeliverableIter::Ordered(map.iter())
            }
        }
    }

    /// Returns the number of messages in the network.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        match self {
            Network::UnorderedDuplicating(set) => set.len(),
            Network::UnorderedNonDuplicating(multiset) => multiset.values().sum(),
            Network::Ordered(map) => map.values().map(VecDeque::len).sum(),
        }
    }

    /// Sends a message.
    pub(crate) fn send(&mut self, envelope: Envelope<Msg>) {
        match self {
            Network::UnorderedDuplicating(set) => {
                set.insert(envelope);
            }
            Network::UnorderedNonDuplicating(multiset) => {
                *multiset.entry(envelope).or_insert(0) += 1;
            }
            Network::Ordered(map) => {
                map.entry((envelope.src, envelope.dst))
                    .or_insert_with(|| VecDeque::with_capacity(1))
                    .push_back(envelope.msg);
            }
        }
    }

    pub(crate) fn on_deliver(&mut self, envelope: Envelope<Msg>)
    where Msg: PartialEq,
    {
        match self {
            Network::UnorderedDuplicating(_) => {
                // This is a no-op as the message can be redelivered.
            }
            Network::UnorderedNonDuplicating(multiset) => {
                match multiset.entry(envelope) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let value = *entry.get();
                        assert!(value > 0);
                        if value == 1 { entry.remove(); }
                        else { *entry.get_mut() -= 1; }
                    }
                    std::collections::hash_map::Entry::Vacant(_) => {
                        panic!("envelope not found");
                    }
                }
            }
            Network::Ordered(map) => {
                // Find the flow, then find the message in the flow, and finally remove the message
                // from the flow. Flows must be non-empty (to ensure removing a message is the
                // inverse of adding it), so also canonicalize by deleting the entire flow if it
                // would be empty after removing the message.
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

    pub(crate) fn on_drop(&mut self, envelope: Envelope<Msg>)
    where Msg: PartialEq,
    {
        match self {
            Network::UnorderedDuplicating(set) => {
                set.remove(&envelope);
            }
            Network::UnorderedNonDuplicating(multiset) => {
                match multiset.entry(envelope) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let value = *entry.get();
                        assert!(value > 0);
                        if value == 1 { entry.remove(); }
                        else { *entry.get_mut() -= 1; }
                    }
                    std::collections::hash_map::Entry::Vacant(_) => {
                        panic!("envelope not found");
                    }
                }
            }
            Network::Ordered(map) => {
                // Find the flow, then find the message in the flow, and finally remove the message
                // from the flow. Flows must be non-empty (to ensure removing a message is the
                // inverse of adding it), so also canonicalize by deleting the entire flow if it
                // would be empty after removing the message.
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
            Network::UnorderedDuplicating(set) => Network::UnorderedDuplicating(set.rewrite(plan)),
            Network::UnorderedNonDuplicating(multiset) =>
                Network::UnorderedNonDuplicating(multiset.rewrite(plan)),
            Network::Ordered(map) => Network::Ordered(map.rewrite(plan)),
        }
    }
}

pub enum NetworkIter<'a, Msg> {
    UnorderedDuplicating(hash_set::Iter<'a, Envelope<Msg>>),
    UnorderedNonDuplicating(
        // active env/count to iterate over repeated sends
        Option<(Envelope<&'a Msg>, usize)>, 
        std::collections::hash_map::Iter<'a, Envelope<Msg>, usize>),
    Ordered(
        // active channel/cursor to iterate over all messages of a channel
        Option<(Id, Id, &'a VecDeque<Msg>, usize)>,
        btree_map::Iter<'a, (Id, Id), VecDeque<Msg>>),
}

impl<'a, Msg> Iterator for NetworkIter<'a, Msg> {
    type Item = Envelope<&'a Msg>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            NetworkIter::UnorderedDuplicating(it) => {
                it.next().map(|env| Envelope {
                    src: env.src,
                    dst: env.dst,
                    msg: &env.msg,
                })
            }
            NetworkIter::UnorderedNonDuplicating(active, it) => {
                if let Some((env, count)) = active { // invariant: count > 1
                    let env = *env; // to avoid holding a reference inside active
                    *count -= 1;
                    if *count == 0 { *active = None; }
                    return Some(env);
                }
                it.next().map(|(env, count)| {
                    let env = Envelope {
                        src: env.src,
                        dst: env.dst,
                        msg: &env.msg,
                    };
                    if *count > 1 { *active = Some((env, *count)); }
                    env
                })
            }
            NetworkIter::Ordered(active, it) => {
                if let Some((src, dst, messages, index)) = active {
                    let msg = messages
                                .get(*index)
                                .unwrap(); // messages.len() > 1
                    return Some(Envelope {
                        src: *src,
                        dst: *dst,
                        msg,
                    });
                }
                it.next().map(|(&(src, dst), messages)| {
                    let msg = messages.get(0).unwrap(); // messages.len() > 1
                    *active = Some((src, dst, messages, 0));
                    Envelope { src, dst, msg }
                })
            }
        }
    }
}

pub enum NetworkDeliverableIter<'a, Msg> {
    UnorderedDuplicating(hash_set::Iter<'a, Envelope<Msg>>),
    UnorderedNonDuplicating(hash_map::Keys<'a, Envelope<Msg>, usize>),
    Ordered(btree_map::Iter<'a, (Id, Id), VecDeque<Msg>>),
}

impl<'a, Msg> Iterator for NetworkDeliverableIter<'a, Msg> {
    type Item = Envelope<&'a Msg>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            NetworkDeliverableIter::UnorderedDuplicating(it) => {
                it.next().map(|env| Envelope {
                    src: env.src,
                    dst: env.dst,
                    msg: &env.msg,
                })
            },
            NetworkDeliverableIter::UnorderedNonDuplicating(it) => {
                it.next().map(|env| Envelope {
                    src: env.src,
                    dst: env.dst,
                    msg: &env.msg,
                })
            },
            NetworkDeliverableIter::Ordered(it) => {
                it.next().map(|(&(src, dst), messages)|{
                    let msg = messages.get(0).expect("empty channel");
                    Envelope { src, dst, msg }
                })
            }
        }
    }
}
