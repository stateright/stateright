//! Models semantics for an actor system on a lossy network that can redeliver messages.

use crate::*;
use crate::actor::*;
use std::sync::Arc;

/// Represents a network of messages.
pub type Network<Msg> = std::collections::BTreeSet<Envelope<Msg>>;

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Copy, Clone, PartialEq)]
pub enum LossyNetwork { Yes, No }

/// Indicates whether the network duplicates messages. If duplication is disabled, messages
/// are forgotten once delivered, which can improve model checking perfomance.
#[derive(Copy, Clone, PartialEq)]
pub enum DuplicatingNetwork { Yes, No }

/// Represents a system of actors that communicate over a network.
/// Usage: `let checker = my_system.into_model().checker()`.
pub trait System: Sized {
    /// The type of actor for this system.
    type Actor: Actor;

    /// Defines the actors.
    fn actors(&self) -> Vec<Self::Actor>;

    /// Defines the initial network.
    fn init_network(&self) -> Vec<Envelope<<Self::Actor as Actor>::Msg>> {
        Vec::with_capacity(20)
    }

    /// Defines whether the network loses messages or not.
    fn lossy_network(&self) -> LossyNetwork {
        LossyNetwork::No
    }

    /// Defines whether the network duplicates messages or not.
    fn duplicating_network(&self) -> DuplicatingNetwork {
        DuplicatingNetwork::Yes
    }

    /// Generates the expected properties for this model.
    fn properties(&self) -> Vec<Property<SystemModel<Self>>>;

    /// Indicates whether a state is within the state space that should be model checked.
    fn within_boundary(&self, _state: &SystemState<Self::Actor>) -> bool {
        true
    }

    /// Converts this system into a model that can be checked.
    fn into_model(self) -> SystemModel<Self> {
        SystemModel {
            actors: self.actors(),
            init_network: self.init_network(),
            lossy_network: self.lossy_network(),
            duplicating_network: self.duplicating_network(),
            system: self,
        }
    }
}

/// A model of an actor system.
#[derive(Clone)]
pub struct SystemModel<S: System> {
    pub actors: Vec<S::Actor>,
    pub init_network: Vec<Envelope<<S::Actor as Actor>::Msg>>,
    pub lossy_network: LossyNetwork,
    pub duplicating_network: DuplicatingNetwork,
    pub system: S,
}

impl<S: System> Model for SystemModel<S> {
    type State = SystemState<S::Actor>;
    type Action = SystemAction<<S::Actor as Actor>::Msg>;

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
                // Early exit if this was a no-op.
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states[index];
                let Event::Receive(src, msg) = event;
                let out = self.actors[index].next_out(id, last_actor_state, Event::Receive(src, msg.clone()));
                if out.state.is_none() && out.commands.is_empty() { return None; }

                // Replace the actor state if changed.
                let mut next_sys_state = last_sys_state.clone();
                if let Some(next_actor_state) = out.state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }

                // If we're a non-duplicating network, drop the message that was delivered.
                if self.duplicating_network == DuplicatingNetwork::No {
                    let env = Envelope { src, dst: id, msg };
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

    fn properties(&self) -> Vec<Property<Self>> {
        self.system.properties()
    }

    fn within_boundary(&self, state: &Self::State) -> bool {
        self.system.within_boundary(state)
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
#[derive(Clone, Debug, PartialEq)]
pub enum SystemAction<Msg> {
    /// An event can be delivered to an actor.
    Act(Id, Event<Msg>),
    /// A message can be dropped if the network is lossy.
    Drop(Envelope<Msg>),
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

        // helper to make the test more concise
        let fingerprint = |states: Vec<PingPongCount>, envelopes: Vec<Envelope<_>>| {
            fingerprint(&_SystemState {
                actor_states: states.into_iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
            })
        };

        let mut checker = PingPongSystem {
            max_nat: 1,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes
        }.into_model().checker();
        assert!(checker.check(1_000).is_done());
        assert_eq!(checker.generated_count(), 14);

        let state_space = checker.generated_fingerprints();
        assert_eq!(state_space.len(), 14); // same as the generated count
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
    fn maintains_fixed_delta_despite_lossy_duplicating_network() {
        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes,
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 4_094);

        assert_eq!(checker.counterexample("delta within 1"), None);
    }

    #[test]
    fn may_never_reach_max_on_lossy_network() {
        use crate::actor::Id;
        use crate::actor::system::SystemAction;
        use crate::test_util::ping_pong::PingPongMsg;

        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes,
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 4_094);

        // can lose the first message and get stuck, for example
        let counterexample = checker.counterexample("reaches max").unwrap();
        assert!(counterexample.last_state().network.is_empty());
        assert_eq!(counterexample.into_actions(), vec![
            SystemAction::Drop(Envelope { src: Id(0), dst: Id(1), msg: PingPongMsg::Ping(0) })
        ]);
    }

    #[test]
    fn eventually_reaches_max_on_perfect_delivery_network() {
        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::No, // important to avoid false negative (liveness checking bug)
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 11);

        assert_eq!(checker.counterexample("reaches max"), None);
    }

    #[test]
    fn can_reach_max() {
        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::Yes,
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 11);

        // this is an example of a safety property that fails to hold as we can reach the max (but not exceed it)
        assert_eq!(
            checker.counterexample("less than max").unwrap().last_state().actor_states,
            vec![Arc::new(PingPongCount(5)), Arc::new(PingPongCount(5))]);
    }

    #[test]
    fn may_never_reach_beyond_max() { // and in fact "will never" (but we're focusing on liveness here)
        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::No, // important to avoid false negative (liveness checking bug)
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 11);

        // this is an example of a liveness property that fails to hold (due to the boundary)
        assert_eq!(
            checker.counterexample("reaches beyond max").unwrap().last_state().actor_states,
            vec![Arc::new(PingPongCount(5)), Arc::new(PingPongCount(5))]);
    }

    #[test]
    fn checker_subject_to_false_negatives_for_liveness_properties() {
        let mut checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::Yes, // this triggers the bug
        }.into_model().checker();
        assert!(checker.check(10_000).is_done());
        assert_eq!(checker.generated_count(), 11);

        // revisits state where liveness property was not yet satisfied and falsely assumes will never be
        assert_eq!(
            checker.counterexample("reaches max").unwrap().last_state().actor_states,
            vec![Arc::new(PingPongCount(0)), Arc::new(PingPongCount(1))]);
    }
}
