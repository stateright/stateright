//! Models semantics for an actor system on a lossy network that can redeliver messages.

use crate::*;
use crate::actor::*;

/// A performant ID type for model checking.
pub type ModelId = usize;

/// Represents a network of messages.
pub type Network<Msg> = std::collections::BTreeSet<Envelope<Msg>>;

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Clone, PartialEq)]
pub enum LossyNetwork { Yes, No }

/// A collection of actors on a lossy network.
#[derive(Clone)]
pub struct ActorSystem<A: Actor<ModelId>> {
    pub init_network: Vec<Envelope<A::Msg>>,
    pub actors: Vec<A>,
    pub lossy_network: LossyNetwork,
}

/// Indicates the source and destination for a message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Envelope<Msg> { pub src: ModelId, pub dst: ModelId, pub msg: Msg }

/// Represents a snapshot in time for the entire actor system.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActorSystemSnapshot<Msg, State> {
    pub actor_states: Vec<Arc<State>>,
    pub network: Network<Msg>,
}

/// Indicates possible steps that an actor system can take as it evolves.
#[derive(Clone, Debug)]
pub enum ActorSystemAction<Msg> {
    Drop(Envelope<Msg>),
    Act(ModelId, ActorInput<ModelId, Msg>),
}

impl<A: Actor<ModelId>> StateMachine for ActorSystem<A>
where
    A::Msg: Clone + Debug + Ord,
    A::State: Clone + Debug + Eq,
{
    type State = ActorSystemSnapshot<A::Msg, A::State>;
    type Action = ActorSystemAction<A::Msg>;

    fn init_states(&self) -> Vec<Self::State> {
        let mut actor_states = Vec::new();
        let mut network = Network::new();

        // init the network
        for e in self.init_network.clone() {
            network.insert(e);
        }

        // init each actor collecting state and messages
        for (src, actor) in self.actors.iter().enumerate() {
            let result = actor.start();
            actor_states.push(Arc::new(result.state));
            for o in result.outputs.0 {
                match o {
                    ActorOutput::Send { dst, msg } => { network.insert(Envelope { src, dst, msg }); },
                }
            }
        }

        vec![ActorSystemSnapshot { actor_states, network }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for env in &state.network {
            // option 1: message is lost
            if self.lossy_network == LossyNetwork::Yes {
                actions.push(ActorSystemAction::Drop(env.clone()));
            }

            // option 2: message is delivered
            let input = ActorInput::Deliver { src: env.src, msg: env.msg.clone() };
            actions.push(ActorSystemAction::Act(env.dst, input));
        }
    }

    fn next_state(&self, last_state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match action {
            ActorSystemAction::Drop(env) => {
                let mut state = last_state.clone();
                state.network.remove(&env);
                Some(state)
            },
            ActorSystemAction::Act(id, input) => {
                if let Some(result) = self.actors[*id].advance(&last_state.actor_states[*id], input) {
                    let mut state = last_state.clone();
                    state.actor_states[*id] = Arc::new(result.state);
                    for output in result.outputs.0 {
                        match output {
                            ActorOutput::Send {dst, msg} => { state.network.insert(Envelope {src: *id, dst, msg}); },
                        }
                    }
                    Some(state)
                } else {
                    None
                }
            },
        }
    }

    fn format_step(&self, last_state: &Self::State, action: &Self::Action) -> String
    where
        Self::Action: Debug,
        Self::State: Debug,
    {
        match action {
            ActorSystemAction::Drop(env) => {
                format!("Dropping {:?}", env)
            },
            ActorSystemAction::Act(id, input) => {
                let last_state = &last_state.actor_states[*id];
                if let Some(ActorResult {
                    state: next_state,
                    outputs: ActorOutputVec(outputs),
                }) = self.actors[*id].advance(last_state, input)
                {
                    let mut description = String::new();
                    {
                        let mut out = |x: String| description.push_str(&x);
                        let invert = |x: String| format!("\x1B[7m{}\x1B[0m", x);

                        // describe action
                        match input {
                            ActorInput::Deliver { src, msg } =>
                                out(invert(format!("{} receives from {} message {:?}", id, src, msg)))
                        }

                        // describe state
                        if *last_state != Arc::new(next_state.clone()) {
                            out(format!("\n  State becomes {}", diff(&last_state, &next_state)));
                        }

                        // describe outputs
                        for output in outputs {
                            match output {
                                ActorOutput::Send { dst, msg } =>
                                    out(format!("\n  Sends {:?} message {:?}", dst, msg))
                            }
                        }
                    }
                    description

                } else {
                    format!("{} ignores {:?}", id, action)
                }
            },
        }
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use crate::actor::*;
    use crate::actor::system::*;

    enum Cfg<Id> {
        Pinger { max_nat: u32, ponger_id: Id },
        Ponger { max_nat: u32 }
    }
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    enum State { Pinger(u32), Ponger(u32) }
    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    enum Msg { Ping(u32), Pong(u32) }

    impl<Id: Copy> Actor<Id> for Cfg<Id> {
        type Msg = Msg;
        type State = State;

        fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
            match self {
                Cfg::Pinger { ponger_id, .. } => ActorResult::start(
                    State::Pinger(0),
                    |outputs| outputs.send(*ponger_id, Msg::Ping(0))),
                Cfg::Ponger { .. } => ActorResult::start(
                    State::Ponger(0),
                    |_outputs| {}),
            }
        }

        fn advance(&self, state: &Self::State, input: &ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>> {
            let ActorInput::Deliver { src, msg } = input.clone();
            match self {
                &Cfg::Pinger { max_nat, .. } => {
                    if let &State::Pinger(actor_value) = state {
                        if let Msg::Pong(msg_value) = msg {
                            if actor_value == msg_value && actor_value < max_nat {
                                return ActorResult::advance(state, |state, outputs| {
                                    *state = State::Pinger(actor_value + 1);
                                    outputs.send(src, Msg::Ping(msg_value + 1));
                                });
                            }
                        }
                    }
                    return None;
                }
                &Cfg::Ponger { max_nat, .. } => {
                    if let &State::Ponger(actor_value) = state {
                        if let Msg::Ping(msg_value) = msg {
                            if actor_value == msg_value && actor_value < max_nat {
                                return ActorResult::advance(state, |state, outputs| {
                                    *state = State::Ponger(actor_value + 1);
                                    outputs.send(src, Msg::Pong(msg_value));
                                });
                            }
                        }
                    }
                    return None;
                }
            }
        }
    }

    fn invariant(_sys: &ActorSystem<Cfg<ModelId>>, state: &ActorSystemSnapshot<Msg, State>) -> bool {
        let &ActorSystemSnapshot { ref actor_states, .. } = state;
        fn extract_value(a: &Arc<State>) -> u32 {
            match **a {
                State::Pinger(value) => value,
                State::Ponger(value) => value,
            }
        };

        let max = actor_states.iter().map(extract_value).max().unwrap();
        let min = actor_states.iter().map(extract_value).min().unwrap();
        max - min <= 1
    }

    #[test]
    fn visits_expected_states() {
        use fxhash::FxHashSet;
        use std::iter::FromIterator;

        let snapshot_hash = |states: Vec<State>, envelopes: Vec<Envelope<_>>| {
            hash(&ActorSystemSnapshot {
                actor_states: states.iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
            })
        };
        let system = ActorSystem {
            actors: vec![
                Cfg::Pinger { max_nat: 1, ponger_id: 1 },
                Cfg::Ponger { max_nat: 1 },
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::Yes,
        };
        let mut checker = system.checker(invariant);
        checker.check(1_000);
        assert_eq!(checker.sources().len(), 14);
        let state_space = FxHashSet::from_iter(checker.sources().keys().cloned());
        assert_eq!(state_space, FxHashSet::from_iter(vec![
            // When the network loses no messages...
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(0)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(0)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::Pinger(0), State::Ponger(1)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(1) }]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::Pinger(1), State::Ponger(1)],
                Vec::new()),
        ]));
    }

    #[test]
    fn can_play_ping_pong() {
        let sys = ActorSystem {
            actors: vec![
                Cfg::Pinger { max_nat: 5, ponger_id: 1 },
                Cfg::Ponger { max_nat: 5 },
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::Yes,
        };
        let mut checker = sys.checker(invariant);
        let result = checker.check(1_000_000);
        assert_eq!(result, CheckResult::Pass);
        assert_eq!(checker.sources().len(), 4094);
    }
}
