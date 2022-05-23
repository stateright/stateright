//! Private module for selective re-export.

use crate::actor::{Actor, Id, Network};
use crate::{Representative, Rewrite, RewritePlan};
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
where
    A: Actor,
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
where
    A: Actor,
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
where
    A: Actor,
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
        self.is_timer_set.hash(state);
        self.network.hash(state);
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
            && self.is_timer_set.eq(&other.is_timer_set)
            && self.network.eq(&other.network)
    }
}

impl<A, H> Representative for ActorModelState<A, H>
where
    A: Actor,
    A::Msg: Rewrite<Id>,
    A::State: Ord + Rewrite<Id>,
    H: Rewrite<Id>,
{
    fn representative(&self) -> Self {
        let plan = RewritePlan::from_values_to_sort(&self.actor_states);
        Self {
            actor_states: plan.reindex(&self.actor_states),
            network: self.network.rewrite(&plan),
            is_timer_set: plan.reindex(&self.is_timer_set),
            history: self.history.rewrite(&plan),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::actor::{Actor, ActorModelState, Envelope, Id, Network, Out};
    use crate::{Representative, Rewrite, RewritePlan};
    use std::sync::Arc;

    #[test]
    fn can_find_representative_from_equivalence_class() {
        let state = ActorModelState::<A, History> {
            actor_states: vec![
                Arc::new(ActorState {
                    acks: vec![Id::from(1), Id::from(2)],
                }),
                Arc::new(ActorState { acks: vec![] }),
                Arc::new(ActorState {
                    acks: vec![Id::from(1)],
                }),
            ],
            network: Network::new_unordered_duplicating([
                // Id(0) sends peers "Write(X)" and receives two acks.
                Envelope {
                    src: 0.into(),
                    dst: 1.into(),
                    msg: "Write(X)",
                },
                Envelope {
                    src: 0.into(),
                    dst: 2.into(),
                    msg: "Write(X)",
                },
                Envelope {
                    src: 1.into(),
                    dst: 0.into(),
                    msg: "Ack(X)",
                },
                Envelope {
                    src: 2.into(),
                    dst: 0.into(),
                    msg: "Ack(X)",
                },
                // Id(2) sends peers "Write(Y)" and receives one ack.
                Envelope {
                    src: 2.into(),
                    dst: 0.into(),
                    msg: "Write(Y)",
                },
                Envelope {
                    src: 2.into(),
                    dst: 1.into(),
                    msg: "Write(Y)",
                },
                Envelope {
                    src: 1.into(),
                    dst: 2.into(),
                    msg: "Ack(Y)",
                },
            ]),
            is_timer_set: vec![true, false, true],
            history: History {
                send_sequence: vec![
                    // Id(0) sends two writes
                    0.into(),
                    0.into(),
                    // Id(2) sends two writes
                    2.into(),
                    2.into(),
                    // Id(2) gets two replies (although only one was delivered)
                    1.into(),
                    0.into(),
                    // Id(0) gets two replies
                    1.into(),
                    2.into(),
                ],
            },
        };
        let representative_state = state.representative();
        // The chosen rewrite plan is:
        // - reindexing: x[0] <- x[1], x[1] <- x[2], x[2] <- x[0]
        // - rewriting:  Id(0) -> Id(2), Id(1) -> Id(0), Id(2) -> Id(1)
        assert_eq!(
            representative_state,
            ActorModelState {
                actor_states: vec![
                    Arc::new(ActorState { acks: vec![] }),
                    Arc::new(ActorState {
                        acks: vec![Id::from(0)]
                    }),
                    Arc::new(ActorState {
                        acks: vec![Id::from(0), Id::from(1)]
                    }),
                ],
                network: Network::new_unordered_duplicating([
                    // Id(2) sends peers "Write(X)" and receives two acks.
                    Envelope {
                        src: 2.into(),
                        dst: 0.into(),
                        msg: "Write(X)"
                    },
                    Envelope {
                        src: 2.into(),
                        dst: 1.into(),
                        msg: "Write(X)"
                    },
                    Envelope {
                        src: 0.into(),
                        dst: 2.into(),
                        msg: "Ack(X)"
                    },
                    Envelope {
                        src: 1.into(),
                        dst: 2.into(),
                        msg: "Ack(X)"
                    },
                    // Id(1) sends peers "Write(Y)" and receives one ack.
                    Envelope {
                        src: 1.into(),
                        dst: 2.into(),
                        msg: "Write(Y)"
                    },
                    Envelope {
                        src: 1.into(),
                        dst: 0.into(),
                        msg: "Write(Y)"
                    },
                    Envelope {
                        src: 0.into(),
                        dst: 1.into(),
                        msg: "Ack(Y)"
                    },
                ]),
                is_timer_set: vec![false, true, true],
                history: History {
                    send_sequence: vec![
                        // Id(2) sends two writes
                        2.into(),
                        2.into(),
                        // Id(1) sends two writes
                        1.into(),
                        1.into(),
                        // Id(1) gets two replies (although only one was delivered)
                        0.into(),
                        2.into(),
                        // Id(2) gets two replies
                        0.into(),
                        1.into(),
                    ],
                },
            }
        );
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
