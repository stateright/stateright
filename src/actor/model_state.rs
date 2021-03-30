//! Private module for selective re-export.

use crate::actor::{Actor, Network};
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Represents a snapshot in time for the entire actor system.
pub struct ActorModelState<A: Actor, H> {
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

