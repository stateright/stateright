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

    fn display_outcome(&self, last_state: &Self::State, action: &Self::Action) -> Option<String>
    where Self::State: Debug
    {
        if let ActorSystemAction::Act(id, input) = action {
            let last_state = &last_state.actor_states[*id];
            let result = self.actors[*id].advance(last_state, input);
            if let Some(ActorResult { state, outputs: ActorOutputVec(outputs) }) = result {
                #[derive(Debug)]
                struct ActorStep<State, Msg> {
                    last_state: Arc<State>,
                    next_state: State,
                    outputs: Vec<ActorOutput<ModelId, Msg>>
                }
                return Some(format!("{:#?}", ActorStep {
                    last_state: last_state.clone(),
                    next_state: state,
                    outputs
                }));
            }
        }
        None
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use crate::actor::system::*;
    use crate::checker::*;
    use crate::test_util::ping_pong::*;
    use std::sync::Arc;

    #[test]
    fn visits_expected_states() {
        use fxhash::FxHashSet;
        use std::iter::FromIterator;

        let fingerprint = |states: Vec<State>, envelopes: Vec<Envelope<_>>| {
            fingerprint(&ActorSystemSnapshot {
                actor_states: states.iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
            })
        };
        let system = ActorSystem {
            actors: vec![
                PingPong::Pinger { max_nat: 1, ponger_id: 1 },
                PingPong::Ponger { max_nat: 1 },
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::Yes,
        };
        let mut checker = Checker::new(&system, invariant);
        checker.check(1_000);
        assert_eq!(checker.sources().len(), 14);
        let state_space = FxHashSet::from_iter(checker.sources().keys().cloned());
        assert_eq!(state_space, FxHashSet::from_iter(vec![
            // When the network loses no messages...
            fingerprint(
                vec![State::Pinger(0), State::Ponger(0)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            fingerprint(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            fingerprint(
                vec![State::Pinger(0), State::Ponger(0)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            fingerprint(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            fingerprint(
                vec![State::Pinger(0), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            fingerprint(
                vec![State::Pinger(0), State::Ponger(1)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(1) }]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            fingerprint(
                vec![State::Pinger(1), State::Ponger(1)],
                Vec::new()),
        ]));
    }

    #[test]
    fn can_play_ping_pong() {
        let sys = ActorSystem {
            actors: vec![
                PingPong::Pinger { max_nat: 5, ponger_id: 1 },
                PingPong::Ponger { max_nat: 5 },
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::Yes,
        };
        let mut checker = Checker::new(&sys, invariant);
        let result = checker.check(1_000_000);
        assert_eq!(result, CheckResult::Pass);
        assert_eq!(checker.sources().len(), 4094);
    }
}
