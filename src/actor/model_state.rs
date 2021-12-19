//! Private module for selective re-export.

use crate::{Reindex, Rewrite, Symmetric};
use crate::actor::{Actor, Id, Network};
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Represents a snapshot in time for the entire actor system.
pub struct ActorModelState<A: Actor, H = ()> {
    pub actor_states: Vec<Arc<A::State>>,
    pub network: Network<A::Msg>,
    pub is_timer_set: Vec<bool>,
    pub history: H,
}

impl<A, H> serde::Serialize for ActorModelState<A, H>
where A: Actor,
      A::State: serde::Serialize,
      A::Msg: serde::Serialize,
      H: serde::Serialize,
{
    fn serialize<Ser: serde::Serializer>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error> {
        use serde::ser::SerializeStruct;
        let mut out = ser.serialize_struct("ActorModelState", 4)?;
        out.serialize_field("actor_states", &self.actor_states)?;
        out.serialize_field("network", &self.network)?;
        out.serialize_field("is_timer_set", &self.is_timer_set)?;
        out.serialize_field("history", &self.history)?;
        out.end()
    }
}
impl<A, H> Symmetric for ActorModelState<A, H>
where A: Actor,
      A::State: Ord + Rewrite<Vec<Id>>,
      H: Rewrite<Vec<Id>>,
{
    fn permutations(&self) -> Box<dyn Iterator<Item=Self> + '_> {
        unimplemented!()
    }
    fn a_sorted_permutation(&self) -> Self {
        // This implementation canonicalizes the actor states by sorting them. It then rewrites IDs
        // and information relating to IDs (such as indices) in every field accordingly.
        let mut combined = self.actor_states.iter()
            .enumerate()
            .map(|(i, s)| (s, i))
            .collect::<Vec<_>>();
        combined.sort_unstable();

        let mapping = combined.iter()
            .map(|(_, i)| Id::from(combined[*i].1))
            .collect();
        Self {
            actor_states: combined.into_iter()
                .map(|(s, _)| Arc::new(s.rewrite(&mapping)))
                .collect(),
            network: self.network.rewrite(&mapping),
            is_timer_set: self.is_timer_set.reindex(&mapping),
            history: self.history.rewrite(&mapping),
        }
    }
}

// Manual implementation to avoid `Clone` constraint that `#derive(Clone)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Clone for ActorModelState<A, H>
where A: Actor,
      H: Clone,
{
    fn clone(&self) -> Self {
        ActorModelState {
            actor_states: self.actor_states.clone(),
            history: self.history.clone(),
            is_timer_set: self.is_timer_set.clone(),
            network: self.network.clone(),
        }
    }
}

// Manual implementation to avoid `Debug` constraint that `#derive(Debug)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Debug for ActorModelState<A, H>
where A: Actor,
      H: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut builder = f.debug_struct("ActorModelState");
        builder.field("actor_states", &self.actor_states);
        builder.field("history", &self.history);
        builder.field("is_timer_set", &self.is_timer_set);
        builder.field("network", &self.network);
        builder.finish()
    }
}

// Manual implementation to avoid `Eq` constraint that `#derive(Eq)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Eq for ActorModelState<A, H>
where A: Actor,
      A::State: Eq,
      H: Eq,
{}

// Manual implementation to avoid `Hash` constraint that `#derive(Hash)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Hash for ActorModelState<A, H>
where A: Actor,
      H: Hash,
{
    fn hash<Hash: Hasher>(&self, state: &mut Hash) {
        self.actor_states.hash(state);
        self.history.hash(state);
        self.is_timer_set.hash(state);
        self.network.hash(state);
    }
}

// Manual implementation to avoid `PartialEq` constraint that `#derive(PartialEq)` would
// introduce on `ActorModelState<A, H>` type parameters.
impl<A, H> PartialEq for ActorModelState<A, H>
where A: Actor,
      A::State: PartialEq,
      H: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.actor_states.eq(&other.actor_states)
            && self.history.eq(&other.history)
            && self.is_timer_set.eq(&other.is_timer_set)
            && self.network.eq(&other.network)
    }
}


#[cfg(test)]
mod test {
    use std::sync::Arc;
    use crate::actor::{Actor, ActorModelState, Envelope, Id, Out};
    use crate::{Rewrite, Symmetric};

    #[test]
    fn can_canonicalize() {
        let state = ActorModelState::<A, History> {
            actor_states: vec![
                Arc::new(ActorState { acks: vec![Id::from(1), Id::from(2)]}),
                Arc::new(ActorState { acks: vec![]}),
                Arc::new(ActorState { acks: vec![Id::from(1)]}),
            ],
            network: vec![
                // Id(0) sends peers "Write(X)" and receives two acks.
                Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },
                Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },

                // Id(2) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(Y)" },
            ].into_iter().collect(),
            is_timer_set: vec![true, false, true],
            history: History {
                send_sequence: vec![
                    // Id(0) sends two writes
                    0.into(), 0.into(),
                    // Id(2) sends two writes
                    2.into(), 2.into(),
                    // Id(2) gets two replies (although only one was delivered)
                    1.into(), 0.into(),
                    // Id(0) gets two replies
                    1.into(), 2.into(),
                ],
            },
        };
        let canonical_state = state.a_sorted_permutation();
        // Chosen "mapping" data structure is [Id(2), Id(0), Id(1)].
        // i.e. Id(0) becomes Id(2), Id(1) becomes Id(0), and Id(2) becomes Id(1).
        assert_eq!(canonical_state, ActorModelState {
            actor_states: vec![
                Arc::new(ActorState { acks: vec![]}),
                Arc::new(ActorState { acks: vec![Id::from(0)]}),
                Arc::new(ActorState { acks: vec![Id::from(0), Id::from(1)]}),
            ],
            network: vec![
                // Id(2) sends peers "Write(X)" and receives two acks.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Ack(Y)" },
            ].into_iter().collect(),
            is_timer_set: vec![true, true, false],
            history: History {
                send_sequence: vec![
                    // Id(2) sends two writes
                    2.into(), 2.into(),
                    // Id(1) sends two writes
                    1.into(), 1.into(),
                    // Id(1) gets two replies (although only one was delivered)
                    0.into(), 2.into(),
                    // Id(2) gets two replies
                    0.into(), 1.into(),
                ],
            },
        });
    }

    struct A;
    impl Actor for A {
        type Msg = &'static str;
        type State = ActorState;
        fn on_start(&self, _id: Id, _o: &mut Out<Self>) -> Self::State {
            unimplemented!();
        }
    }

    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    struct ActorState { acks: Vec<Id> }
    impl Rewrite<Vec<Id>> for ActorState {
        fn rewrite(&self, mapping: &Vec<Id>) -> Self {
            Self { acks: self.acks.rewrite(mapping) }
        }
    }

    #[derive(Debug, PartialEq)]
    struct History { send_sequence: Vec<Id> }
    impl Rewrite<Vec<Id>> for History {
        fn rewrite(&self, mapping: &Vec<Id>) -> Self {
            Self { send_sequence: self.send_sequence.rewrite(mapping) }
        }
    }
}
