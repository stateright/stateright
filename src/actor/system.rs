//! Private module for selective re-export.

use crate::*;
use crate::actor::*;
use crate::util::HashableHashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

/// Represents a network of messages.
pub type Network<Msg> = HashableHashSet<Envelope<Msg>>;

/// Indicates whether the network loses messages. Note that as long as invariants do not check
/// the network state, losing a message is indistinguishable from an unlimited delay, so in
/// many cases you can improve model checking performance by not modeling message loss.
#[derive(Copy, Clone, PartialEq)]
pub enum LossyNetwork { Yes, No }

/// Indicates whether the network duplicates messages. If duplication is disabled, messages
/// are forgotten once delivered, which can improve model checking performance.
#[derive(Copy, Clone, PartialEq)]
pub enum DuplicatingNetwork { Yes, No }

/// Represents a system of actors that communicate over a network.
/// Usage: `let checker = my_system.into_model().checker()`.
pub trait System: Sized {
    /// The type of actor for this system.
    type Actor: Actor;

    /// The type of history to maintain as auxiliary state, if any.
    /// See [Auxiliary Variables in TLA](https://lamport.azurewebsites.net/tla/auxiliary/auxiliary.html)
    /// for a thorough introduction to that concept. Use `()` if history is not needed to define
    /// the relevant properties of this system.
    type History: Clone + Debug + Default + Hash;

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

    /// Defines whether/how an incoming message contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    fn record_msg_in(&self, history: &Self::History, src: Id, dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        let _ = history;
        let _ = src;
        let _ = dst;
        let _ = msg;
        None
    }

    /// Defines whether/how an outgoing messages contributes to relevant history. Returning
    /// `Some(new_history)` updates the relevant history, while `None` does not.
    fn record_msg_out(&self, history: &Self::History, src: Id, dst: Id, msg: &<Self::Actor as Actor>::Msg) -> Option<Self::History> {
        let _ = history;
        let _ = src;
        let _ = dst;
        let _ = msg;
        None
    }

    /// Generates the expected properties for this model.
    fn properties(&self) -> Vec<Property<SystemModel<Self>>>;

    /// Indicates whether a state is within the state space that should be model checked.
    fn within_boundary(&self, _state: &SystemState<Self>) -> bool {
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
    type State = SystemState<S>;
    type Action = SystemAction<<S::Actor as Actor>::Msg>;

    fn init_states(&self) -> Vec<Self::State> {
        let mut init_sys_state = SystemState {
            actor_states: Vec::with_capacity(self.actors.len()),
            network: Network::with_hasher(stable::build_hasher()), // for consistent discoveries
            is_timer_set: Vec::new(),
            history: S::History::default(),
        };

        // init the network
        for e in self.init_network.clone() {
            init_sys_state.network.insert(e);
        }

        // init each actor
        for (index, actor) in self.actors.iter().enumerate() {
            let id = Id::from(index);
            let out = actor.on_start_out(id);
            init_sys_state.actor_states.push(
                Arc::new(
                    out.state.unwrap_or_else(|| panic!("on_start must assign state. id={:?}", id))));
            self.process_commands(id, out.commands, &mut init_sys_state);
        }

        vec![init_sys_state]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for env in &state.network {
            // option 1: message is lost
            if self.lossy_network == LossyNetwork::Yes {
                actions.push(SystemAction::Drop(env.clone()));
            }

            // option 2: message is delivered
            if usize::from(env.dst) < self.actors.len() {
                actions.push(SystemAction::Deliver { src: env.src, dst: env.dst, msg: env.msg.clone() });
            }
        }

        // option 3: actor timeout
        for (index, &is_scheduled) in state.is_timer_set.iter().enumerate() {
            if is_scheduled {
                actions.push(SystemAction::Timeout(Id::from(index)));
            }
        }
    }

    fn next_state(&self, last_sys_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            SystemAction::Drop(env) => {
                let mut next_state = last_sys_state.clone();
                next_state.network.remove(&env);
                Some(next_state)
            },
            SystemAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states.get(index);

                // Not all messags can be delivered, so ignore those.
                if last_actor_state.is_none() { return None; }
                let last_actor_state = last_actor_state.unwrap();

                // Some operations are no-ops, so ignore those as well.
                let out = self.actors[index].on_msg_out(id, last_actor_state, src, msg.clone());
                if out.is_no_op() { return None; }
                let history = self.system.record_msg_in(&last_sys_state.history, src, id, &msg);

                // Update the state as necessary:
                // - Drop delivered message if not a duplicating network.
                // - Swap out revised actor state.
                // - Track message input history.
                // - Handle effect of commands on timers, network, and message output history.
                let mut next_sys_state = last_sys_state.clone();
                if self.duplicating_network == DuplicatingNetwork::No {
                    // Strictly speaking, this state should be updated regardless of whether the
                    // actor and history updates are a no-op. The current implementation is only
                    // safe if invariants do not relate to the existence of envelopes on the
                    // network.
                    let env = Envelope { src, dst: id, msg };
                    next_sys_state.network.remove(&env);
                }
                if let Some(next_actor_state) = out.state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                if let Some(history) = history {
                    next_sys_state.history = history;
                }
                self.process_commands(id, out.commands, &mut next_sys_state);
                Some(next_sys_state)
            },
            SystemAction::Timeout(id) => {
                // Clone new state if necessary (otherwise early exit).
                let index = usize::from(id);
                let last_actor_state = &last_sys_state.actor_states[index];
                let out = self.actors[index].on_timeout_out(id, last_actor_state);
                let keep_timer = out.commands.iter()
                    .any(|c| matches!(c, Command::SetTimer(_)));
                if out.is_no_op() && keep_timer { return None; }
                let mut next_sys_state = last_sys_state.clone();

                // Timer is no longer valid.
                next_sys_state.is_timer_set[index] = false;

                if let Some(next_actor_state) = out.state {
                    next_sys_state.actor_states[index] = Arc::new(next_actor_state);
                }
                self.process_commands(id, out.commands, &mut next_sys_state);
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

        match action {
            SystemAction::Drop(_) => {
                None
            },
            SystemAction::Deliver { src, dst: id, msg } => {
                let index = usize::from(id);
                if let Some(actor_state) = &last_state.actor_states.get(index) {
                    let out = self.actors[index].on_msg_out(id, actor_state, src, msg);
                    Some(format!("{:#?}", ActorStep {
                        last_state: actor_state,
                        next_state: out.state,
                        commands: out.commands,
                    }))
                } else {
                    None
                }
            },
            SystemAction::Timeout(id) => {
                let index = usize::from(id);
                let actor_state = &last_state.actor_states[index];
                let out = self.actors[index].on_timeout_out(id, actor_state);
                Some(format!("{:#?}", ActorStep {
                    last_state: actor_state,
                    next_state: out.state,
                    commands: out.commands,
                }))
            },
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        self.system.properties()
    }

    fn within_boundary(&self, state: &Self::State) -> bool {
        self.system.within_boundary(state)
    }
}

impl<S: System> SystemModel<S> {
    /// Updates the actor state, sends messages, and configures the timer.
    fn process_commands(&self, id: Id, commands: Vec<Command<<S::Actor as Actor>::Msg>>, state: &mut SystemState<S>) {
        let index = usize::from(id);
        for c in commands {
            match c {
                Command::Send(dst, msg) => {
                    if let Some(history) = self.system.record_msg_out(&state.history, id, dst, &msg) {
                        state.history = history;
                    }
                    state.network.insert(Envelope { src: id, dst, msg });
                },
                Command::SetTimer(_) => {
                    // must use the index to infer how large as actor state may not be initialized yet
                    if state.is_timer_set.len() <= index {
                        state.is_timer_set.resize(index + 1, false);
                    }
                    state.is_timer_set[index] = true;
                },
                Command::CancelTimer => {
                    state.is_timer_set[index] = false;
                },
            }
        }
    }
}

/// Indicates the source and destination for a message.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(serde::Serialize)]
pub struct Envelope<Msg> { pub src: Id, pub dst: Id, pub msg: Msg }

/// Represents a snapshot in time for the entire actor system.
pub struct SystemState<S: System> {
    pub actor_states: Vec<Arc<<S::Actor as Actor>::State>>,
    pub network: Network<<S::Actor as Actor>::Msg>,
    pub is_timer_set: Vec<bool>,
    pub history: S::History,
}

impl<S> serde::Serialize for SystemState<S>
where S: System,
      <S::Actor as Actor>::State: serde::Serialize,
      <S::Actor as Actor>::Msg: serde::Serialize,
      S::History: serde::Serialize,
{
    fn serialize<Ser: serde::Serializer>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error> {
        use serde::ser::SerializeStruct;
        let mut out = ser.serialize_struct("SystemState", 4)?;
        out.serialize_field("actor_states", &self.actor_states)?;
        out.serialize_field("network", &self.network)?;
        out.serialize_field("is_timer_set", &self.is_timer_set)?;
        out.serialize_field("history", &self.history)?;
        out.end()
    }
}

// Manual implementation to avoid `S: Clone` constraint that `#derive(Clone)` would introduce on
// `SystemState<S>`.
impl<S: System> Clone for SystemState<S> {
    fn clone(&self) -> Self {
        SystemState {
            actor_states: self.actor_states.clone(),
            network: self.network.clone(),
            is_timer_set: self.is_timer_set.clone(),
            history: self.history.clone(),
        }
    }
}

// Manual implementation to avoid `S: Debug` constraint that `#derive(Debug)` would introduce on
// `SystemState<S>`.
impl<S: System> Debug for SystemState<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut builder = f.debug_struct("SystemState");
        builder.field("actor_states", &self.actor_states);
        builder.field("history", &self.history);
        builder.field("is_timer_set", &self.is_timer_set);
        builder.field("network", &self.network);
        builder.finish()
    }
}

// Manual implementation to avoid `S: Eq` constraint that `#derive(Eq)` would introduce on
// `SystemState<S>`.
impl<S: System> Eq for SystemState<S>
where <S::Actor as Actor>::State: Eq, S::History: Eq {}

// Manual implementation to avoid `S: Hash` constraint that `#derive(Hash)` would introduce on
// `SystemState<S>`.
impl<S: System> Hash for SystemState<S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.actor_states.hash(state);
        self.history.hash(state);
        self.is_timer_set.hash(state);
        self.network.hash(state);
    }
}

// Manual implementation to avoid `S: PartialEq` constraint that `#derive(PartialEq)` would introduce on
// `SystemState<S>`.
impl<S: System> PartialEq for SystemState<S>
where <S::Actor as Actor>::State: PartialEq, S::History: PartialEq {
    fn eq(&self, other: &Self) -> bool {
        self.actor_states.eq(&other.actor_states)
            && self.history.eq(&other.history)
            && self.is_timer_set.eq(&other.is_timer_set)
            && self.network.eq(&other.network)
    }
}

/// Indicates possible steps that an actor system can take as it evolves.
#[derive(Clone, Debug, PartialEq)]
pub enum SystemAction<Msg> {
    /// A message can be delivered to an actor.
    Deliver { src: Id, dst: Id, msg: Msg },
    /// A message can be dropped if the network is lossy.
    Drop(Envelope<Msg>),
    /// An actor can by notified after a timeout.
    Timeout(Id),
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

/// The specific timeout value is not relevant for model checking, so this helper can be used to
/// generate an arbitrary timeout range. The specific value is subject to change, so this helper
/// must only be used for model checking.
pub fn model_timeout() -> Range<Duration> {
    Duration::from_micros(0)..Duration::from_micros(0)
}

/// A helper to generate a list of peer [`Id`]s given an actor count and the index of a particular
/// actor.
pub fn model_peers(self_ix: usize, count: usize) -> Vec<Id> {
    (0..count)
        .filter(|j| *j != self_ix)
        .map(Into::into)
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::actor::actor_test_util::ping_pong::{PingPongCount, PingPongMsg::*, PingPongSystem};
    use crate::actor::system::SystemAction::*;
    use std::collections::HashSet;
    use std::sync::Arc;


    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;

        // helper to make the test more concise
        let states_and_network = |states: Vec<PingPongCount>, envelopes: Vec<Envelope<_>>| {
            SystemState::<PingPongSystem> {
                actor_states: states.into_iter().map(|s| Arc::new(s)).collect::<Vec<_>>(),
                network: Network::from_iter(envelopes),
                is_timer_set: Vec::new(),
                history: (0_u32, 0_u32), // constant as `maintains_history: false`
            }
        };

        let (recorder, accessor) = StateRecorder::new_with_accessor();
        let checker = PingPongSystem {
            max_nat: 1,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes,
            maintains_history: false,
        }.into_model().checker().visitor(recorder).spawn_bfs().join();
        assert_eq!(checker.generated_count(), 14);

        let state_space = accessor();
        assert_eq!(state_space.len(), 14); // same as the generated count
        assert_eq!(HashSet::<_>::from_iter(state_space), HashSet::from_iter(vec![
            // When the network loses no messages...
            states_and_network(
                vec![PingPongCount(0), PingPongCount(0)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            states_and_network(
                vec![PingPongCount(0), PingPongCount(0)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            states_and_network(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }]),
            states_and_network(
                vec![PingPongCount(0), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![PingPongCount(0), PingPongCount(1)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) },
                ]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![
                    Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) },
                    Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) },
                ]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(1) }]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(1), dst: Id::from(0), msg: Pong(0) }]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                vec![Envelope { src: Id::from(0), dst: Id::from(1), msg: Ping(0) }]),
            states_and_network(
                vec![PingPongCount(1), PingPongCount(1)],
                Vec::new()),
        ]));
    }

    #[test]
    fn maintains_fixed_delta_despite_lossy_duplicating_network() {
        let checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes,
            maintains_history: false,
        }.into_model().checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 4_094);
        checker.assert_no_discovery("delta within 1");
    }

    #[test]
    fn may_never_reach_max_on_lossy_network() {
        let checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::Yes,
            duplicating: DuplicatingNetwork::Yes,
            maintains_history: false,
        }.into_model().checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 4_094);

        // can lose the first message and get stuck, for example
        checker.assert_discovery("must reach max", vec![
            Drop(Envelope { src: Id(0), dst: Id(1), msg: Ping(0) }),
        ]);
    }

    #[test]
    fn eventually_reaches_max_on_perfect_delivery_network() {
        let checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::No,
            maintains_history: false,
        }.into_model().checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);
        checker.assert_no_discovery("must reach max");
    }

    #[test]
    fn can_reach_max() {
        let checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::Yes,
            maintains_history: false,
        }.into_model().checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);
        assert_eq!(
            checker.discovery("can reach max").unwrap().last_state().actor_states,
            vec![Arc::new(PingPongCount(4)), Arc::new(PingPongCount(5))]);
    }

    #[test]
    fn might_never_reach_beyond_max() {
        // ^ and in fact will never. This is a subtle distinction: we're exercising a
        //   falsifiable liveness property here (eventually must exceed max), whereas "will never"
        //   refers to a verifiable safety property (always will not exceed).

        let checker = PingPongSystem {
            max_nat: 5,
            lossy: LossyNetwork::No,
            duplicating: DuplicatingNetwork::No,
            maintains_history: false,
        }.into_model().checker().spawn_bfs().join();
        assert_eq!(checker.generated_count(), 11);

        // this is an example of a liveness property that fails to hold (due to the boundary)
        assert_eq!(
            checker.discovery("must exceed max").unwrap().last_state().actor_states,
            vec![Arc::new(PingPongCount(5)), Arc::new(PingPongCount(5))]);
    }

    #[test]
    fn handles_undeliverable_messages() {
        struct TestActor;
        impl Actor for TestActor {
            type State = ();
            type Msg = ();
            fn on_start(&self, _: Id, o: &mut Out<Self>) { o.set_state(()); }
            fn on_msg(&self, _: Id, _: &Self::State, _: Id, _: Self::Msg, _: &mut Out<Self>) {}
        }
        struct TestSystem;
        impl System for TestSystem {
            type Actor = TestActor;
            type History = ();
            fn actors(&self) -> Vec<Self::Actor> { Vec::new() }
            fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
                // need one property, otherwise checking early exits
                vec![Property::always("unused", |_, _| true)]
            }
            fn init_network(&self) -> Vec<Envelope<<Self::Actor as Actor>::Msg>> {
                vec![Envelope { src: 0.into(), dst: 99.into(), msg: () }]
            }
        }
        assert!(TestSystem.into_model().checker().spawn_bfs().join().is_done());
    }

    #[test]
    fn resets_timer() {
        struct TestActor;
        impl Actor for TestActor {
            type State = ();
            type Msg = ();
            fn on_start(&self, _: Id, o: &mut Out<Self>) {
                o.set_state(());
                o.set_timer(model_timeout());
            }
            fn on_msg(&self, _: Id, _: &Self::State, _: Id, _: Self::Msg, _: &mut Out<Self>) {}
        }
        struct TestSystem;
        impl System for TestSystem {
            type Actor = TestActor;
            type History = ();
            fn actors(&self) -> Vec<Self::Actor> { vec![TestActor] }
            fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
                vec![Property::always("unused", |_, _| true)]
            }
        }
        // Init state with timer, followed by next state without timer.
        assert_eq!(2, TestSystem.into_model().checker().spawn_bfs().join().generated_count());
    }
}
