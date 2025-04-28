//! Utilities for actor tests.

/// A pair of actors that send messages and increment message counters.
pub mod ping_pong {
    use crate::actor::*;
    use crate::*;

    pub struct PingPongActor {
        serve_to: Option<Id>,
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub enum PingPongMsg {
        Ping(u32),
        Pong(u32),
    }

    impl Actor for PingPongActor {
        type Msg = PingPongMsg;
        type State = u32; // count
        type Timer = ();
        type Random = ();
        type Storage = ();
        fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
            if let Some(id) = self.serve_to {
                o.send(id, PingPongMsg::Ping(0));
            }
            0
        }

        fn on_msg(
            &self,
            _id: Id,
            state: &mut Cow<Self::State>,
            src: Id,
            msg: Self::Msg,
            o: &mut Out<Self>,
        ) {
            match msg {
                PingPongMsg::Pong(msg_value) if **state == msg_value => {
                    o.send(src, PingPongMsg::Ping(msg_value + 1));
                    *state.to_mut() += 1;
                }
                PingPongMsg::Ping(msg_value) if **state == msg_value => {
                    o.send(src, PingPongMsg::Pong(msg_value));
                    *state.to_mut() += 1;
                }
                _ => {}
            }
        }
    }

    pub struct PingPongCfg {
        pub maintains_history: bool,
        pub max_nat: u32,
    }

    pub type PingPongHistory = (u32, u32); // (#in, #out)

    impl PingPongCfg {
        pub fn into_model(self) -> ActorModel<PingPongActor, PingPongCfg, PingPongHistory> {
            ActorModel::new(self, (0, 0))
                .actor(PingPongActor {
                    serve_to: Some(1.into()),
                })
                .actor(PingPongActor { serve_to: None })
                .record_msg_in(|cfg, history, _| {
                    if cfg.maintains_history {
                        let (msg_in_count, msg_out_count) = *history;
                        Some((msg_in_count + 1, msg_out_count))
                    } else {
                        None
                    }
                })
                .record_msg_out(|cfg, history, _| {
                    if cfg.maintains_history {
                        let (msg_in_count, msg_out_count) = *history;
                        Some((msg_in_count, msg_out_count + 1))
                    } else {
                        None
                    }
                })
                .within_boundary(|cfg, state| {
                    state
                        .actor_states
                        .iter()
                        .all(|count| **count <= cfg.max_nat)
                })
                .property(Expectation::Always, "delta within 1", |_, state| {
                    let max = state.actor_states.iter().max().unwrap();
                    let min = state.actor_states.iter().min().unwrap();
                    **max - **min <= 1
                })
                .property(Expectation::Sometimes, "can reach max", |model, state| {
                    state
                        .actor_states
                        .iter()
                        .any(|count| **count == model.cfg.max_nat)
                })
                .property(Expectation::Eventually, "must reach max", |model, state| {
                    state
                        .actor_states
                        .iter()
                        .any(|count| **count == model.cfg.max_nat)
                })
                .property(
                    Expectation::Eventually,
                    "must exceed max",
                    |model, state| {
                        // this one is falsifiable due to the boundary
                        state
                            .actor_states
                            .iter()
                            .any(|count| **count == model.cfg.max_nat + 1)
                    },
                )
                .property(Expectation::Always, "#in <= #out", |_, state| {
                    let (msg_in_count, msg_out_count) = state.history;
                    msg_in_count <= msg_out_count
                })
                .property(Expectation::Eventually, "#out <= #in + 1", |_, state| {
                    let (msg_in_count, msg_out_count) = state.history;
                    msg_out_count <= msg_in_count + 1
                })
        }
    }
}
