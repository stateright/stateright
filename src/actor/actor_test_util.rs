//! Utilities for actor tests.

/// A pair of actors that send messages and increment message counters.
pub mod ping_pong {
    use crate::*;
    use crate::actor::*;
    use crate::actor::system::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum PingPongActor { PingActor { pong_id: Id }, PongActor }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct PingPongCount(pub u32);

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub enum PingPongMsg { Ping(u32), Pong(u32) }

    impl Actor for PingPongActor {
        type Msg = PingPongMsg;
        type State = PingPongCount;

        fn on_start(&self, _id: Id, o: &mut Out<Self>) {
            o.set_state(PingPongCount(0));
            if let PingPongActor::PingActor { pong_id } = self {
                o.send(*pong_id, PingPongMsg::Ping(0));
            }
        }

        fn on_msg(&self, _id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
            match (self, msg) {
                (PingPongActor::PingActor { .. }, PingPongMsg::Pong(msg_value)) if state.0 == msg_value => {
                    o.set_state(PingPongCount(state.0 + 1));
                    o.send(src, PingPongMsg::Ping(msg_value + 1));
                },
                (PingPongActor::PongActor, PingPongMsg::Ping(msg_value)) if state.0 == msg_value => {
                    o.set_state(PingPongCount(state.0 + 1));
                    o.send(src, PingPongMsg::Pong(msg_value));
                },
                _ => {},
            }
        }
    }

    #[derive(Clone)]
    pub struct PingPongSystem {
        pub max_nat: u32,
        pub lossy: LossyNetwork,
        pub duplicating: DuplicatingNetwork,
    }

    impl System for PingPongSystem {
        type Actor = PingPongActor;

        fn actors(&self) -> Vec<Self::Actor> {
            vec![
                PingPongActor::PingActor { pong_id: Id::from(1) },
                PingPongActor::PongActor,
            ]
        }

        fn lossy_network(&self) -> LossyNetwork { self.lossy }

        fn duplicating_network(&self) -> DuplicatingNetwork { self.duplicating }

        fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
            vec![
                Property::<SystemModel<Self>>::always("delta within 1", |_, state| {
                    let max = state.actor_states.iter().map(|s| s.0).max().unwrap();
                    let min = state.actor_states.iter().map(|s| s.0).min().unwrap();
                    max - min <= 1
                }),
                Property::<SystemModel<Self>>::always("less than max", |model, state| {
                    // this one is falsifiable at the boundary
                    state.actor_states.iter().any(|s| s.0 < model.system.max_nat)
                }),
                Property::<SystemModel<Self>>::eventually("reaches max", move |model, state| {
                    state.actor_states.iter().any(|s| s.0 == model.system.max_nat)
                }),
                Property::<SystemModel<Self>>::eventually("reaches beyond max", move |model, state| {
                    // this one is falsifiable due to the boundary
                    state.actor_states.iter().any(|s| s.0 == model.system.max_nat + 1)
                }),
            ]
        }

        fn within_boundary(&self, state: &SystemState<Self::Actor>) -> bool {
            state.actor_states.iter().all(|s| s.0 <= self.max_nat)
        }
    }
}
