//! Private module for selective re-export.

use serde::Serialize;

use crate::actor::{Actor, Id, Network};
use crate::util::HashableHashMap;
use crate::{Representative, Rewrite, RewritePlan};
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::timers::Timers;

/// Represents a snapshot in time for the entire actor system.
pub struct ActorModelState<A: Actor, H = ()> {
    pub actor_states: Vec<Arc<A::State>>,
    pub network: Network<A::Msg>,
    pub timers_set: Vec<Timers<A::Timer>>,
    pub random_choices: Vec<RandomChoices<A::Random>>,
    pub crashed: Vec<bool>,
    pub history: H,
    pub actor_storages: Vec<Option<A::Storage>>,
}

/// Represents a set of random choices for one actor.
#[derive(Clone, Debug, Serialize, Hash, Eq, PartialEq)]
pub struct RandomChoices<Random> {
    /// The map of random choices for an actor.
    ///
    /// The string key is the key given in [`Actor::choose_random`], and the value vec contains the
    /// possible random choices to select from.
    pub map: HashableHashMap<String, Vec<Random>>,
}

impl<Random> Default for RandomChoices<Random> {
    fn default() -> Self {
        Self {
            map: Default::default(),
        }
    }
}

impl<Random> RandomChoices<Random> {
    pub fn insert(&mut self, key: String, choices: Vec<Random>) {
        self.map.insert(key, choices);
    }

    pub fn remove(&mut self, key: &String) -> Option<Vec<Random>> {
        self.map.remove(key)
    }
}

impl<Random: Rewrite<Id>> Rewrite<Id> for RandomChoices<Random> {
    fn rewrite<S>(&self, plan: &RewritePlan<Id, S>) -> Self {
        Self {
            map: self
                .map
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().map(|r| r.rewrite(plan)).collect()))
                .collect::<HashableHashMap<_, _, _>>(),
        }
    }
}

impl<A, H> serde::Serialize for ActorModelState<A, H>
where
    A: Actor,
    A::State: serde::Serialize,
    A::Msg: serde::Serialize,
    A::Timer: serde::Serialize,
    A::Random: serde::Serialize,
    A::Storage: serde::Serialize,
    H: serde::Serialize,
{
    fn serialize<Ser: serde::Serializer>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error> {
        use serde::ser::SerializeStruct;
        let mut out = ser.serialize_struct("ActorModelState", 7)?;
        out.serialize_field("actor_states", &self.actor_states)?;
        out.serialize_field("network", &self.network)?;
        out.serialize_field("timers_set", &self.timers_set)?;
        out.serialize_field("random_choices", &self.random_choices)?;
        out.serialize_field("crashed", &self.crashed)?;
        out.serialize_field("history", &self.history)?;
        out.serialize_field("storages", &self.actor_storages)?;
        out.end()
    }
}

// Manual implementation to avoid `Clone` constraint that `#derive(Clone)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Clone for ActorModelState<A, H>
where
    A: Actor,
    H: Clone,
{
    fn clone(&self) -> Self {
        ActorModelState {
            actor_states: self.actor_states.clone(),
            history: self.history.clone(),
            timers_set: self.timers_set.clone(),
            random_choices: self.random_choices.clone(),
            network: self.network.clone(),
            crashed: self.crashed.clone(),
            actor_storages: self.actor_storages.clone(),
        }
    }
}

// Manual implementation to avoid `Debug` constraint that `#derive(Debug)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Debug for ActorModelState<A, H>
where
    A: Actor,
    H: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut builder = f.debug_struct("ActorModelState");
        builder.field("actor_states", &self.actor_states);
        builder.field("history", &self.history);
        builder.field("timers_set", &self.timers_set);
        builder.field("random_choices", &self.random_choices);
        builder.field("network", &self.network);
        builder.field("crashed", &self.crashed);
        builder.field("storages", &self.actor_storages);
        builder.finish()
    }
}

// Manual implementation to avoid `Eq` constraint that `#derive(Eq)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Eq for ActorModelState<A, H>
where
    A: Actor,
    A::State: Eq,
    H: Eq,
{
}

// Manual implementation to avoid `Hash` constraint that `#derive(Hash)` would introduce on
// `ActorModelState<A, H>` type parameters.
impl<A, H> Hash for ActorModelState<A, H>
where
    A: Actor,
    H: Hash,
{
    fn hash<Hash: Hasher>(&self, state: &mut Hash) {
        self.actor_states.hash(state);
        self.history.hash(state);
        self.timers_set.hash(state);
        self.random_choices.hash(state);
        self.network.hash(state);
        self.crashed.hash(state);
        self.actor_storages.hash(state);
    }
}

// Manual implementation to avoid `PartialEq` constraint that `#derive(PartialEq)` would
// introduce on `ActorModelState<A, H>` type parameters.
impl<A, H> PartialEq for ActorModelState<A, H>
where
    A: Actor,
    A::State: PartialEq,
    H: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.actor_states.eq(&other.actor_states)
            && self.history.eq(&other.history)
            && self.timers_set.eq(&other.timers_set)
            && self.random_choices.eq(&other.random_choices)
            && self.network.eq(&other.network)
            && self.crashed.eq(&other.crashed)
            && self.actor_storages.eq(&other.actor_storages)
    }
}

impl<A, H> Representative for ActorModelState<A, H>
where
    A: Actor,
    A::Msg: Rewrite<Id>,
    A::State: Ord + Rewrite<Id>,
    A::Random: Rewrite<Id>,
    A::Storage: Rewrite<Id>,
    H: Rewrite<Id>,
{
    fn representative(&self) -> Self {
        let plan = RewritePlan::from_values_to_sort(&self.actor_states);
        Self {
            actor_states: plan.reindex(&self.actor_states),
            network: self.network.rewrite(&plan),
            timers_set: plan.reindex(&self.timers_set),
            random_choices: plan.reindex(&self.random_choices),
            crashed: plan.reindex(&self.crashed),
            actor_storages: plan.reindex(&self.actor_storages),
            history: self.history.rewrite(&plan),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::actor::timers::Timers;
    use crate::actor::{Actor, ActorModelState, Envelope, Id, Network, Out, RandomChoices};
    use crate::{Representative, Rewrite, RewritePlan};
    use std::sync::Arc;

    #[test]
    fn can_find_representative_from_equivalence_class() {
        let empty_timers = Timers::new();
        let mut non_empty_timers = Timers::new();
        non_empty_timers.set(());
        #[rustfmt::skip]
        let state = ActorModelState::<A, History> {
            actor_states: vec![
                Arc::new(ActorState { acks: vec![Id::from(1), Id::from(2)]}),
                Arc::new(ActorState { acks: vec![]}),
                Arc::new(ActorState { acks: vec![Id::from(1)]}),
            ],
            network: Network::new_unordered_duplicating([
                // Id(0) sends peers "Write(X)" and receives two acks.
                Envelope { src: 0.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Write(X)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Ack(X)" },
                Envelope { src: 2.into(), dst: 0.into(), msg: "Ack(X)" },

                // Id(2) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(Y)" },
            ]),
            timers_set: vec![non_empty_timers.clone(), empty_timers.clone(), non_empty_timers.clone()],
            random_choices: vec![RandomChoices::default(); 3],
            crashed: vec![false; 3],
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
            actor_storages: vec![None; 3],
        };
        let representative_state = state.representative();
        // The chosen rewrite plan is:
        // - reindexing: x[0] <- x[1], x[1] <- x[2], x[2] <- x[0]
        // - rewriting:  Id(0) -> Id(2), Id(1) -> Id(0), Id(2) -> Id(1)
        #[rustfmt::skip]
        assert_eq!(representative_state, ActorModelState {
            actor_states: vec![
                Arc::new(ActorState { acks: vec![]}),
                Arc::new(ActorState { acks: vec![Id::from(0)]}),
                Arc::new(ActorState { acks: vec![Id::from(0), Id::from(1)]}),
            ],
            network: Network::new_unordered_duplicating([
                // Id(2) sends peers "Write(X)" and receives two acks.
                Envelope { src: 2.into(), dst: 0.into(), msg: "Write(X)" },
                Envelope { src: 2.into(), dst: 1.into(), msg: "Write(X)" },
                Envelope { src: 0.into(), dst: 2.into(), msg: "Ack(X)" },
                Envelope { src: 1.into(), dst: 2.into(), msg: "Ack(X)" },

                // Id(1) sends peers "Write(Y)" and receives one ack.
                Envelope { src: 1.into(), dst: 2.into(), msg: "Write(Y)" },
                Envelope { src: 1.into(), dst: 0.into(), msg: "Write(Y)" },
                Envelope { src: 0.into(), dst: 1.into(), msg: "Ack(Y)" },
            ]),
            timers_set: vec![empty_timers, non_empty_timers.clone(), non_empty_timers.clone()],
            random_choices: vec![RandomChoices::default(); 3],
            crashed: vec![false; 3],
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
        actor_storages: vec![None; 3],});
    }

    struct A;
    impl Actor for A {
        type Msg = &'static str;
        type State = ActorState;
        type Timer = ();
        type Random = ();
        type Storage = ();
        fn on_start(
            &self,
            _id: Id,
            _storage: &Option<Self::Storage>,
            _o: &mut Out<Self>,
        ) -> Self::State {
            unimplemented!();
        }
    }

    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    struct ActorState {
        acks: Vec<Id>,
    }
    impl Rewrite<Id> for ActorState {
        fn rewrite<S>(&self, plan: &RewritePlan<Id, S>) -> Self {
            Self {
                acks: self.acks.rewrite(plan),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct History {
        send_sequence: Vec<Id>,
    }
    impl Rewrite<Id> for History {
        fn rewrite<S>(&self, plan: &RewritePlan<Id, S>) -> Self {
            Self {
                send_sequence: self.send_sequence.rewrite(plan),
            }
        }
    }
}
