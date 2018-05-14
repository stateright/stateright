//! This module provides an actor abstraction. See the `model` submodule for a state machine
//! implementation that can check a system of actors.
//!
//! ## Example
//!
//! ```
//! use stateright::*;
//! use stateright::actor::*;
//! use stateright::actor::model::*;
//! use std::iter::FromIterator;
//!
//! struct ClockActor;
//!
//! impl<Id: Copy> Actor<Id> for ClockActor {
//!     type Msg = u32;
//!     type State = u32;
//!
//!     fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
//!         ActorResult::new(0)
//!     }
//!
//!     fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
//!         let ActorInput::Deliver { src, msg: timestamp } = input;
//!         if timestamp > actor.state {
//!             actor.state = timestamp;
//!             actor.send(src, timestamp + 1);
//!         }
//!     }
//! }
//!
//! let sys = ActorSystem {
//!     actors: vec![ClockActor, ClockActor],
//!     init_network: vec![Envelope { src: 1, dst: 0, msg: 1 }],
//! };
//! let mut checker = sys.checker(
//!     true,
//!     |sys, snapshot| snapshot.actor_states.iter().all(|s| *s < 3));
//! assert_eq!(
//!     checker.check(100),
//!     CheckResult::Fail {
//!         state: ActorSystemSnapshot {
//!             actor_states: vec![3, 2],
//!             network: Network::from_iter(vec![
//!                 Envelope { src: 1, dst: 0, msg: 1 },
//!                 Envelope { src: 0, dst: 1, msg: 2 },
//!                 Envelope { src: 1, dst: 0, msg: 3 },
//!                 Envelope { src: 0, dst: 1, msg: 4 },
//!             ]),
//!         }
//!     });
//! ```


use std::hash::Hash;

/// Inputs to which an actor can respond.
pub enum ActorInput<Id, Msg> {
    Deliver { src: Id, msg: Msg },
}

/// Outputs with which an actor can respond.
#[derive(Clone, Debug)]
pub enum ActorOutput<Id, Msg> {
    Send { dst: Id, msg: Msg },
}

/// Packages up the action, state, and outputs for an actor step.
#[derive(Debug)]
pub struct ActorResult<Id, Msg, State> {
    pub action: &'static str,
    pub state: State,
    pub outputs: Vec<ActorOutput<Id, Msg>>,
}

impl<Id, Msg, State> ActorResult<Id, Msg, State> {
    pub fn new(state: State) -> Self {
        ActorResult { action: "actor step", state, outputs: Vec::new() }
    }
    pub fn send(&mut self, dst: Id, msg: Msg) {
        self.outputs.push(ActorOutput::Send { dst, msg })
    }
}

/// An actor initializes internal state optionally emitting outputs; then it waits for incoming
/// events, responding by updating its internal state and optionally emitting outputs.  At the
/// moment, the only inputs and outputs relate to messages, but other events like timers will
/// likely be added.
pub trait Actor<Id> {
    /// The type of messages sent and received by this actor.
    type Msg: Clone + Eq + Hash + Ord;

    /// The type of state maintained by this actor.
    type State: Clone + Eq + Hash + Ord;

    /// Indicates the initial state and outputs for the actor.
    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State>;

    /// Indicates the updated state and outputs for the actor when it receives an input.
    fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>);
}

/// Models semantics for an actor system on a lossy network that can redeliver messages.
pub mod model {
    use ::*;
    use ::actor::*;
    use std::hash::Hash;

    /// A performant ID type for model checking.
    pub type ModelId = usize;

    /// Represents a network of messages.
    pub type Network<Msg> = std::collections::BTreeSet<Envelope<Msg>>;

    /// A collection of actors on a lossy network.
    pub struct ActorSystem<A: Actor<ModelId>> {
        pub init_network: Vec<Envelope<A::Msg>>,
        pub actors: Vec<A>,
    }

    /// Indicates the source and destination for a message.
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct Envelope<Msg> { pub src: ModelId, pub dst: ModelId, pub msg: Msg }

    /// Represents a snapshot in time for the entire actor system.
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct ActorSystemSnapshot<Msg: Hash + Eq + Clone + Ord, State: Hash + Eq + Clone + Ord> {
         pub actor_states: Vec<State>,
         pub network: Network<Msg>,
    }

    impl<A: Actor<ModelId>> StateMachine for ActorSystem<A> {
        type State = ActorSystemSnapshot<A::Msg, A::State>;

        fn init(&self, results: &mut StepVec<Self::State>) {
            let mut actor_states = Vec::new();
            let mut network = Network::new();

            // init the network
            for e in self.init_network.clone() {
                network.insert(e);
            }

            // init each actor collecting state and messages
            for (src, actor) in self.actors.iter().enumerate() {
                let result = actor.start();
                actor_states.push(result.state);
                for o in result.outputs {
                    match o {
                        ActorOutput::Send { dst, msg } => { network.insert(Envelope { src, dst, msg }); },
                    }
                }
            }

            results.push(("INIT", ActorSystemSnapshot { actor_states, network }));
        }

        fn next(&self, state: &Self::State, results: &mut StepVec<Self::State>) {
            for env in &state.network {
                let id = env.dst;

                // option 1: message is lost
                let mut message_lost = state.clone();
                message_lost.network.remove(env);
                results.push(("message lost", message_lost));

                // option 2: message is delivered
                let mut result = ActorResult::new(state.actor_states[id].clone());
                self.actors[id].advance(
                    ActorInput::Deliver { src: env.src, msg: env.msg.clone() },
                    &mut result);
                let mut message_delivered = state.clone();
                message_delivered.actor_states[id] = result.state;
                for output in result.outputs {
                    match output {
                        ActorOutput::Send {dst, msg} => { message_delivered.network.insert(Envelope {src: id, dst, msg}); },
                    }
                }
                results.push((result.action, message_delivered));
            }
        }
    }

#[cfg(test)]
mod test {
    use ::*;
    use ::actor::*;
    use ::actor::model::*;

    struct PingPongActor<Id> { max_nat: u32, role: PingPongActorRole, ponger_id: Id }
    enum PingPongActorRole { Pinger, Ponger }
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    enum PingPongActorMsg { Ping(u32), Pong(u32) }
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    enum PingPongActorState { PingerState(u32), PongerState(u32) }
    impl<Id: Copy> Actor<Id> for PingPongActor<Id> {
        type Msg = PingPongActorMsg;
        type State = PingPongActorState;

        fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
            match self.role {
                PingPongActorRole::Pinger => {
                    let mut result = ActorResult::new(PingPongActorState::PingerState(0));
                    result.send(self.ponger_id, PingPongActorMsg::Ping(0));
                    result
                },
                PingPongActorRole::Ponger => ActorResult::new(PingPongActorState::PongerState(0)),
            }
        }

        fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
            let ActorInput::Deliver { src, msg } = input;
            match (msg, &actor.state) {
                (PingPongActorMsg::Pong(msg_value), &PingPongActorState::PingerState(value)) => {
                    if value == msg_value && value < self.max_nat {
                        actor.state = PingPongActorState::PingerState(value + 1);
                        actor.send(src, PingPongActorMsg::Ping(value + 1));
                    }
                },
                (PingPongActorMsg::Ping(msg_value), &PingPongActorState::PongerState(value)) => {
                    if value == msg_value && value < self.max_nat {
                        actor.state = PingPongActorState::PongerState(value + 1);
                        actor.send(src, PingPongActorMsg::Pong(value));
                    }
                },
                _ => {}
            }
        }
    }
    fn invariant(_sys: &ActorSystem<PingPongActor<ModelId>>, state: &ActorSystemSnapshot<PingPongActorMsg, PingPongActorState>) -> bool {
        let &ActorSystemSnapshot { ref actor_states, .. } = state;
        fn extract_value(a: &PingPongActorState) -> u32 {
            match a {
                &PingPongActorState::PingerState(value) => value,
                &PingPongActorState::PongerState(value) => value,
            }
        };

        let max = actor_states.iter().map(extract_value).max().unwrap();
        let min = actor_states.iter().map(extract_value).min().unwrap();
        max - min <= 1
    }

    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;
        let system = ActorSystem {
            actors: vec![
                PingPongActor { max_nat: 1, role: PingPongActorRole::Pinger, ponger_id: 1 },
                PingPongActor { max_nat: 1, role: PingPongActorRole::Ponger, ponger_id: 1 },
            ],
            init_network: Vec::new(),
        };
        let mut checker = system.checker(true, invariant);
        checker.check(1_000);
        assert_eq!(checker.visited.len(), 14);
        assert_eq!(checker.visited, FxHashSet::from_iter(vec![
            // When the network loses no messages...
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(0)],
                network: Network::from_iter(vec![Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) }]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(1) },
                ]),
            }),

            // When the network loses the message for pinger-ponger state (0, 0)...
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(0)],
                network:  Network::<Envelope<PingPongActorMsg>>::new(),
            }),

            // When the network loses a message for pinger-ponger state (0, 1)
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(0), PingPongActorState::PongerState(1)],
                network:  Network::<Envelope<PingPongActorMsg>>::new(),
            }),

            // When the network loses a message for pinger-ponger state (1, 1)
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(1) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(1) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(1) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 1, dst: 0, msg: PingPongActorMsg::Pong(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network: Network::from_iter(vec![
                    Envelope { src: 0, dst: 1, msg: PingPongActorMsg::Ping(0) },
                ]),
            }),
            hash(&ActorSystemSnapshot {
                actor_states: vec![PingPongActorState::PingerState(1), PingPongActorState::PongerState(1)],
                network:  Network::<Envelope<PingPongActorMsg>>::new(),
            }),
        ]));
    }

    #[test]
    fn can_play_ping_pong() {
        let sys = ActorSystem {
            actors: vec![
                PingPongActor { max_nat: 5, role: PingPongActorRole::Pinger, ponger_id: 1 },
                PingPongActor { max_nat: 5, role: PingPongActorRole::Ponger, ponger_id: 1 },
            ],
            init_network: Vec::new(),
        };
        let mut checker = sys.checker(false, invariant);
        let result = checker.check(1_000_000);
        assert_eq!(result, CheckResult::Pass);
        assert_eq!(checker.visited.len(), 4094);
    }
}
}

