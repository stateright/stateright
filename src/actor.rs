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
//! use std::sync::Arc;
//!
//! struct ClockActor;
//!
//! impl<Id: Copy> Actor<Id> for ClockActor {
//!     type Msg = u32;
//!     type State = u32;
//!
//!     fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
//!         ActorResult::start(0, |_outputs| {})
//!     }
//!
//!     fn advance(&self, state: &Self::State, input: &ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>> {
//!         let ActorInput::Deliver { src, msg: timestamp } = input;
//!         if timestamp > state {
//!             return ActorResult::advance(state, |state, outputs| {
//!                 *state = *timestamp;
//!                 outputs.send(*src, timestamp + 1);
//!             });
//!         }
//!         return None;
//!     }
//! }
//!
//! let sys = ActorSystem {
//!     actors: vec![ClockActor, ClockActor],
//!     init_network: vec![Envelope { src: 1, dst: 0, msg: 1 }],
//!     lossy_network: LossyNetwork::Yes,
//! };
//! let mut checker = sys.checker(
//!     |sys, snapshot| snapshot.actor_states.iter().all(|s| **s < 3));
//! assert_eq!(
//!     checker.check(100),
//!     CheckResult::Fail {
//!         state: ActorSystemSnapshot {
//!             actor_states: vec![Arc::new(3), Arc::new(2)],
//!             network: Network::from_iter(vec![
//!                 Envelope { src: 1, dst: 0, msg: 1 },
//!                 Envelope { src: 0, dst: 1, msg: 2 },
//!                 Envelope { src: 1, dst: 0, msg: 3 },
//!                 Envelope { src: 0, dst: 1, msg: 4 },
//!             ]),
//!         }
//!     });
//! ```

pub mod register;

use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use std::fmt::Debug;
use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;
use std::thread;

/// Inputs to which an actor can respond.
#[derive(Clone, Debug)]
pub enum ActorInput<Id, Msg> {
    Deliver { src: Id, msg: Msg },
}

/// Outputs with which an actor can respond.
#[derive(Clone, Debug)]
pub enum ActorOutput<Id, Msg> {
    Send { dst: Id, msg: Msg },
}

/// We create a wrapper type so we can add convenience methods.
#[derive(Clone, Debug)]
pub struct ActorOutputVec<Id, Msg>(pub Vec<ActorOutput<Id, Msg>>);

impl<Id, Msg> ActorOutputVec<Id, Msg> {
    pub fn send(&mut self, dst: Id, msg: Msg) {
        let ActorOutputVec(outputs) = self;
        outputs.push(ActorOutput::Send { dst, msg })
    }

    pub fn broadcast(&mut self, dsts: &[Id], msg: &Msg)
    where
        Id: Clone,
        Msg: Clone,
    {
        for id in dsts {
            self.send(id.clone(), msg.clone());
        }
    }
}

/// Packages up the state and outputs for an actor step.
#[derive(Debug)]
pub struct ActorResult<Id, Msg, State> {
    pub state: State,
    pub outputs: ActorOutputVec<Id, Msg>,
}

impl<Id, Msg, State> ActorResult<Id, Msg, State> {
    /// Helper for creating a starting result.
    pub fn start<M>(state: State, mutation: M) -> Self
    where M: Fn(&mut ActorOutputVec<Id, Msg>) -> ()
    {
        let mut outputs = ActorOutputVec(Vec::new());
        mutation(&mut outputs);
        ActorResult { state, outputs }
    }

    /// Helper for creating a subsequent result.
    pub fn advance<M>(state: &State, mutation: M) -> Option<Self>
    where
        State: Clone,
        M: Fn(&mut State, &mut ActorOutputVec<Id, Msg>) -> ()
    {
        let mut state = state.clone();
        let mut outputs = ActorOutputVec(Vec::new());
        mutation(&mut state, &mut outputs);
        Some(ActorResult { state, outputs })
    }
}

/// An actor initializes internal state optionally emitting outputs; then it waits for incoming
/// events, responding by updating its internal state and optionally emitting outputs.  At the
/// moment, the only inputs and outputs relate to messages, but other events like timers will
/// likely be added.
pub trait Actor<Id> {
    /// The type of messages sent and received by this actor.
    type Msg;

    /// The type of state maintained by this actor.
    type State;

    /// Indicates the initial state and outputs for the actor.
    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State>;

    /// Indicates the updated state and outputs for the actor when it receives an input.
    fn advance(&self, state: &Self::State, input: &ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>>;

    /// Indicates how to deserialize messages received by a spawned actor.
    fn deserialize(&self, bytes: &[u8]) -> serde_json::Result<Self::Msg> where Self::Msg: DeserializeOwned {
        serde_json::from_slice(bytes)
    }

    /// Indicates how to serialize messages sent by a spawned actor.
    fn serialize(&self, msg: &Self::Msg) -> serde_json::Result<Vec<u8>> where Self::Msg: Serialize {
        serde_json::to_vec(msg)
    }

    /// The effect to perform in response to spawned actor outputs.
    fn on_output(&self, output: &ActorOutput<SpawnId, Self::Msg>, id: &SpawnId, socket: &UdpSocket)
    where Self::Msg: Debug + Serialize
    {
        let ActorOutput::Send { dst, msg } = output;
        match self.serialize(msg) {
            Err(e) => {
                println!("Unable to serialize. Ignoring. id={}, dst={}, msg={:?}, err={}",
                         fmt(id), fmt(dst), msg, e);
            },
            Ok(out_buf) => {
                if let Err(e) = socket.send_to(&out_buf, &dst) {
                    println!("Unable to send. Ignoring. id={}, dst={}, msg={:?}, err={}",
                             fmt(id), fmt(dst), msg, e);
                }
            },
        }
    }
}

/// An ID type for an actor identified by an IP address and port.
pub type SpawnId = (IpAddr, u16);

fn fmt(id: &SpawnId) -> String { format!("{}:{}", id.0, id.1) }

/// Runs an actor by mapping messages to JSON over UDP.
pub fn spawn<A>(actor: A, id: SpawnId) -> thread::JoinHandle<()>
where
    A: 'static + Send + Actor<SpawnId>,
    A::Msg: Debug + DeserializeOwned + Serialize,
    A::State: Debug,
{
    // note that panics are returned as `Err` when `join`ing
    thread::spawn(move || {
        let socket = UdpSocket::bind(id).unwrap(); // panic if unable to bind
        let mut in_buf = [0; 65_535];

        let mut result = actor.start();
        println!("Actor started. id={}, result={:#?}", fmt(&id), result);
        for o in &result.outputs.0 { actor.on_output(o, &id, &socket); }

        loop {
            let (count, src_addr) = socket.recv_from(&mut in_buf).unwrap(); // panic if unable to read
            match actor.deserialize(&in_buf[..count]) {
                Ok(msg) => {
                    println!("Received message. id={}, src={}, msg={:?}", fmt(&id), src_addr, msg);

                    let input = ActorInput::Deliver { src: (src_addr.ip(), src_addr.port()), msg };
                    if let Some(new_result) = actor.advance(&result.state, &input) {
                        println!("Actor advanced. id={}, result={:#?}", fmt(&id), new_result);
                        result = new_result;
                        for o in &result.outputs.0 { actor.on_output(o, &id, &socket); }
                    }
                },
                Err(e) => {
                    println!("Unable to parse message. Ignoring. id={}, src={}, buf={:?}, err={}",
                            fmt(&id), src_addr, &in_buf[..count], e);
                }
            }
        }
    })
}

/// Models semantics for an actor system on a lossy network that can redeliver messages.
pub mod model {
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
    use crate::actor::model::*;

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
}

