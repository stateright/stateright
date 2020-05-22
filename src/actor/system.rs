//! Models semantics for an actor system on a lossy network that can redeliver messages.

use crate::*;
use crate::actor::*;
use std::sync::Arc;

/// Represents a network of messages.
pub type Network<Msg> = std::collections::BTreeSet<Envelope<Msg>>;

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Clone, PartialEq)]
pub enum LossyNetwork { Yes, No }

/// Indicates whether the network duplicates messages. If duplication is disabled, messages
/// are forgotten once delivered, which can improve model checking perfomance.
#[derive(Clone, PartialEq)]
pub enum DuplicatingNetwork { Yes, No }

/// A collection of actors on a lossy network.
#[derive(Clone)]
pub struct System<A: Actor> {
    pub actors: Vec<A>,
    pub init_network: Vec<Envelope<A::Msg>>,
    pub lossy_network: LossyNetwork,
    pub duplicating_network: DuplicatingNetwork,
}

impl<A: Actor> System<A> {
    pub fn with_actors(actors: Vec<A>) -> Self {
        System {
            actors,
            init_network: Vec::new(),
            lossy_network: LossyNetwork::No,
            duplicating_network: DuplicatingNetwork::Yes
        }
    }
}

/// Indicates the source and destination for a message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Envelope<Msg> { pub src: Id, pub dst: Id, pub msg: Msg }

/// Represents a snapshot in time for the entire actor system. Consider using
/// `SystemState<Actor>` instead for simpler type signatures.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct _SystemState<Msg, State> {
    pub actor_states: Vec<Arc<State>>,
    pub network: Network<Msg>,
}

/// A type alias that accepts an actor type and resolves the associated `Msg` and `State`
/// types. Introduced because parameterizing `_SystemState` by `Actor`
/// necessitates implementing extra traits.
pub type SystemState<A> = _SystemState<<A as Actor>::Msg, <A as Actor>::State>;

/// Indicates possible steps that an actor system can take as it evolves.
#[derive(Debug, PartialEq)]
pub enum SystemAction<Msg> {
    Drop(Envelope<Msg>),
    Act(Id, Event<Msg>),
}

impl<A> StateMachine for System<A>
where
    A: Actor,
    A::State: Clone + Debug,
    A::Msg: Clone + Debug + Ord,
{
    type State = SystemState<A>;
    type Action = SystemAction<A::Msg>;

    fn init_states(&self) -> Vec<Self::State> {
        let mut actor_states = Vec::new();
        let mut network = Network::new();

        // init the network
        for e in self.init_network.clone() {
            network.insert(e);
        }

        // init each actor collecting state and messages
        for (index, actor) in self.actors.iter().enumerate() {
            let id = Id::from(index);
            let out = actor.init_out(id);
            actor_states.push(Arc::new(out.state.expect(&format!(
                "Actor state not assigned at init. id={:?}", id))));
            for c in out.commands {
                match c {
                    Command::Send(dst, msg) => {
                        network.insert(Envelope { src: id, dst, msg });
                    },
                }
            }
        }

        vec![_SystemState { actor_states, network }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for env in &state.network {
            // option 1: message is lost
            if self.lossy_network == LossyNetwork::Yes {
                actions.push(SystemAction::Drop(env.clone()));
            }

            // option 2: message is delivered
            let event = Event::Receive(env.src, env.msg.clone());
            actions.push(SystemAction::Act(env.dst, event));
        }
    }

    fn next_state(&self, last_sys_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            SystemAction::Drop(env) => {
                let mut next_state = last_sys_state.clone();
                next_state.network.remove(&env);
                Some(next_state)
            },
            SystemAction::Act(id, event) => {
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states[index];
                let Event::Receive(src, msg) = event;
                let out = self.actors[index].next_out(id, last_actor_state, Event::Receive(src, msg.clone()));
                if out.state.is_none() && out.commands.is_empty() { return None; } // optimization
                let mut next_sys_state = last_sys_state.clone();
                if let Some(next_actor_state) = out.state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                // If we're a non-duplicating nework, drop the message that was delivered.
                if self.duplicating_network == DuplicatingNetwork::No {
                    let env = Envelope { src: src, dst: id, msg: msg };
                    next_sys_state.network.remove(&env);
                }
                // Then insert the messages that were generated in response.
                for c in out.commands {
                    match c {
                        Command::Send(dst, msg) => {
                            next_sys_state.network.insert(Envelope { src: id, dst, msg });
                        },
                    }
                }
                Some(next_sys_state)
            },
        }
    }

    fn display_outcome(&self, last_state: &Self::State, action: Self::Action) -> Option<String>
    where Self::State: Debug
    {
        #[derive(Debug)]
        struct ActorStep<'a, State, Msg> {
            last_state: &'a Arc<State>,
            next_state: Option<State>,
            commands: Vec<Command<Msg>>,
        }

        if let SystemAction::Act(id, event) = action {
            let index = usize::from(id);
            let actor_state = &last_state.actor_states[index];
            let out = self.actors[index].next_out(id, actor_state, event);
            Some(format!("{:#?}", ActorStep {
                last_state: actor_state,
                next_state: out.state,
                commands: out.commands,
            }))
        } else {
            None
        }
    }
}

impl From<Id> for usize {
    fn from(id: Id) -> Self {
        id.0 as usize
    }
}

impl From<usize> for Id {
    fn from(u: usize) -> Self {
        Id(u as u64)
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use crate::actor::system::*;
    use crate::test_util::ping_pong::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;

        let fingerprint = |states: Vec<PingPongCount>, envelopes: Vec<Envelope<_>>| {
            fingerprint(&_SystemState {
                actor_states: states.into_iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
            })
        };
        let mut checker = System {
            actors: vec![
                PingPong::PingActor { pong_id: Id::from(1) },
                PingPong::PongActor,
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::Yes,
            duplicating_network: DuplicatingNetwork::Yes
        }.model(1).checker();
        checker.check(1_000);
        let state_space = checker.generated_fingerprints();
        assert_eq!(state_space.len(), 14);
        assert_eq!(state_space, HashSet::from_iter(vec![
            // When the network loses no messages...
            fingerprint(
                vec![PingPongCount(0), PingPongCount(0)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) }]),
            fingerprint(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) },
                ]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            fingerprint(
                vec![PingPongCount(0), PingPongCount(0)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            fingerprint(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) }]),
            fingerprint(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) }]),
            fingerprint(
                vec![PingPongCount(0), PingPongCount(1)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(1) },
                ]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(1) },
                ]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) },
                ]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(1) }]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: PingPongMsg::Pong(0) }]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: PingPongMsg::Ping(0) }]),
            fingerprint(
                vec![PingPongCount(1), PingPongCount(1)],
                Vec::new()),
        ]));
    }

    #[test]
    fn can_play_ping_pong() {
        let mut checker = System {
            actors: vec![
                PingPong::PingActor { pong_id: Id::from(1) },
                PingPong::PongActor,
            ],
            init_network: Vec::new(),
            lossy_network: LossyNetwork::No,
            duplicating_network: DuplicatingNetwork::No,
        }.model(5).checker();
        assert_eq!(checker.check(10_000).counterexample("delta within 1"), None);
        assert_eq!(checker.check(10_000).counterexample("max_nat"), None);
        assert_ne!(checker.check(10_000).counterexample("max_nat_plus_one"), None);
        assert_eq!(checker.is_done(), true);
        assert_eq!(checker.generated_count(), 11);
    }
}
