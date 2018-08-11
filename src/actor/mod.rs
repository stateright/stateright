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
//! use std::rc::Rc;
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
//!     fn advance(&self, state: &Self::State, input: ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>> {
//!         let ActorInput::Deliver { src, msg: timestamp } = input;
//!         if timestamp > *state {
//!             return ActorResult::advance(state, |action, state, outputs| {
//!                 *action = "clock tick";
//!                 *state = timestamp;
//!                 outputs.send(src, timestamp + 1);
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
//!     KeepPaths::Yes,
//!     |sys, snapshot| snapshot.actor_states.iter().all(|s| **s < 3));
//! assert_eq!(
//!     checker.check(100),
//!     CheckResult::Fail {
//!         state: ActorSystemSnapshot {
//!             actor_states: vec![Rc::new(3), Rc::new(2)],
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

use serde::de::*;
use serde::ser::*;
use serde_json;
use std::fmt::Debug;
use std::net::{IpAddr, UdpSocket};
use std::rc::Rc;
use std::thread;

/// Inputs to which an actor can respond.
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

/// Packages up the action, state, and outputs for an actor step.
#[derive(Debug)]
pub struct ActorResult<Id, Msg, State> {
    pub action: &'static str,
    pub state: State,
    pub outputs: ActorOutputVec<Id, Msg>,
}

impl<Id, Msg, State> ActorResult<Id, Msg, State> {
    /// Helper for creating a starting result.
    pub fn start<M>(state: State, mutation: M) -> Self
    where M: Fn(&mut ActorOutputVec<Id, Msg>) -> ()
    {
        let action = "(INIT)";
        let mut outputs = ActorOutputVec(Vec::new());
        mutation(&mut outputs);
        ActorResult { action, state, outputs }
    }

    /// Helper for creating a subsequent result.
    pub fn advance<M>(state: &State, mutation: M) -> Option<Self>
    where
        State: Clone,
        M: Fn(&mut &'static str, &mut State, &mut ActorOutputVec<Id, Msg>) -> ()
    {
        let mut action = "(ACTION NOT SPECIFIED)";
        let mut state = state.clone();
        let mut outputs = ActorOutputVec(Vec::new());
        mutation(&mut action, &mut state, &mut outputs);
        Some(ActorResult { action, state, outputs })
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
    fn advance(&self, state: &Self::State, input: ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>>;
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
        handle_outputs(&result.outputs, &id, &socket);

        loop {
            let (count, src_addr) = socket.recv_from(&mut in_buf).unwrap(); // panic if unable to read
            let msg: A::Msg = match serde_json::from_slice(&in_buf[..count]) {
                Ok(v) => {
                    println!("Received message. id={}, src={}, msg={:?}", fmt(&id), src_addr, v);
                    v
                },
                Err(e) => {
                    println!("Unable to parse message. Ignoring. id={}, src={}, buf={:?}, err={}",
                            fmt(&id), src_addr, &in_buf[..count], e);
                    continue
                }
            };
            if let Some(new_result) = actor.advance(
                    &result.state,
                    ActorInput::Deliver { src: (src_addr.ip(), src_addr.port()), msg }) {
                println!("Actor advanced. id={}, result={:#?}", fmt(&id), new_result);
                handle_outputs(&new_result.outputs, &id, &socket);
                result = new_result;
            }
        }
    })
}

fn handle_outputs<Msg>(
    outputs: &ActorOutputVec<SpawnId, Msg>, id: &SpawnId, socket: &UdpSocket)
where Msg: Debug + Serialize
{
    for o in &outputs.0 {
        let ActorOutput::Send { dst, msg } = o;
        let out_buf = match serde_json::to_vec(msg) {
            Ok(v) => v,
            Err(e) => {
                println!("Unable to serialize. Ignoring. id={}, dst={}, msg={:?}, err={}",
                         fmt(id), fmt(dst), msg, e);
                continue
            },
        };
        match socket.send_to(&out_buf, &dst) {
            Ok(_) => {}
            Err(e) => {
                println!("Unable to send. Ignoring. id={}, dst={}, msg={:?}, err={}",
                         fmt(id), fmt(dst), msg, e);
                continue
            }
        }
    }
}

/// Provides a DSL to eliminate some boilerplate for defining an actor.
#[macro_export]
macro_rules! actor {
    (
    Cfg $(<$tcfg:ident>)* { $($cfg:tt)* }
    State $(<$tstate:ident>)* { $($state:tt)* }
    Msg $(<$tmsg:ident>)* { $($msg:tt)* }
    Start() { $($start:tt)* }
    Advance($state_advance:ident, $src:ident, $msg_advance:ident) { $($advance:tt)* }
    ) => (
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum Cfg $(<$tcfg>)* { $($cfg)* }

        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum State $(<$tstate>)* { $($state)* }

        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[derive(Serialize, Deserialize)]
        pub enum Msg $(<$tmsg>)* { $($msg)* }

        impl<Id: Copy> Actor<Id> for Cfg $(<$tcfg>)* {
            type Msg = Msg $(<$tmsg>)*;
            type State = State $(<$tstate>)*;

            fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
                match self {
                    $($start)*
                }
            }

            fn advance(&self, $state_advance: &Self::State, input: ActorInput<Id, Self::Msg>) -> Option<ActorResult<Id, Self::Msg, Self::State>> {
                let ActorInput::Deliver { $src, $msg_advance } = input;
                match self {
                    $($advance)*
                }
            }
        }
    )
}

/// Models semantics for an actor system on a lossy network that can redeliver messages.
pub mod model {
    use ::*;
    use ::actor::*;

    /// A performant ID type for model checking.
    pub type ModelId = usize;

    /// Represents a network of messages.
    pub type Network<Msg> = std::collections::BTreeSet<Envelope<Msg>>;

    /// Indicates whether the network loses messages. Note that as long as invariants do not check
    /// the network state, losing a message is indistinguishable from an unlimited delay, so in
    /// many cases you can improve model checking performance by not modeling message loss.
    #[derive(PartialEq)]
    pub enum LossyNetwork { Yes, No }

    /// A collection of actors on a lossy network.
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
        pub actor_states: Vec<Rc<State>>,
        pub network: Network<Msg>,
    }

    impl<A: Actor<ModelId>> StateMachine for ActorSystem<A>
    where
        A::Msg: Clone + Ord,
        A::State: Clone,
    {
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
                actor_states.push(Rc::new(result.state));
                for o in result.outputs.0 {
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
                if self.lossy_network == LossyNetwork::Yes {
                    let mut message_lost = state.clone();
                    message_lost.network.remove(env);
                    results.push(("message lost", message_lost));
                }

                // option 2: message is delivered
                if let Some(result) = self.actors[id].advance(
                        &state.actor_states[id],
                        ActorInput::Deliver { src: env.src, msg: env.msg.clone() }) {
                    let mut message_delivered = state.clone(); 
                    message_delivered.actor_states[id] = Rc::new(result.state);
                    for output in result.outputs.0 {
                        match output {
                            ActorOutput::Send {dst, msg} => { message_delivered.network.insert(Envelope {src: id, dst, msg}); },
                        }
                    }
                    results.push((result.action, message_delivered));
                }
            }
        }
    }

#[cfg(test)]
mod test {
    use ::*;
    use ::actor::*;
    use ::actor::model::*;

    actor! {
        Cfg<Id> { Pinger { max_nat: u32, ponger_id: Id } , Ponger { max_nat: u32 } }
        State { PingerState(u32), PongerState(u32) }
        Msg { Ping(u32), Pong(u32) }
        Start() {
            Cfg::Pinger { ponger_id, .. } => ActorResult::start(
                State::PingerState(0),
                |outputs| outputs.send(*ponger_id, Msg::Ping(0))),
            Cfg::Ponger { .. } => ActorResult::start(
                State::PongerState(0),
                |_outputs| {}),
        }
        Advance(state, src, msg) {
            &Cfg::Pinger { max_nat, .. } => {
                if let &State::PingerState(actor_value) = state {
                    if let Msg::Pong(msg_value) = msg {
                        if actor_value == msg_value && actor_value < max_nat {
                            return ActorResult::advance(state, |action, state, outputs| {
                                *action = "ping";
                                *state = State::PingerState(actor_value + 1);
                                outputs.send(src, Msg::Ping(msg_value + 1));
                            });
                        }
                    }
                }
                return None;
            }
            &Cfg::Ponger { max_nat, .. } => {
                if let &State::PongerState(actor_value) = state {
                    if let Msg::Ping(msg_value) = msg {
                        if actor_value == msg_value && actor_value < max_nat {
                            return ActorResult::advance(state, |action, state, outputs| {
                                *action = "pong";
                                *state = State::PongerState(actor_value + 1);
                                outputs.send(src, Msg::Pong(msg_value));
                            });
                        }
                    }
                }
                return None;
            }
        }
    }

    fn invariant(_sys: &ActorSystem<Cfg<ModelId>>, state: &ActorSystemSnapshot<Msg, State>) -> bool {
        let &ActorSystemSnapshot { ref actor_states, .. } = state;
        fn extract_value(a: &Rc<State>) -> u32 {
            match **a {
                State::PingerState(value) => value,
                State::PongerState(value) => value,
            }
        };

        let max = actor_states.iter().map(extract_value).max().unwrap();
        let min = actor_states.iter().map(extract_value).min().unwrap();
        max - min <= 1
    }

    #[test]
    fn visits_expected_states() {
        use std::iter::FromIterator;
        let snapshot_hash = |states: Vec<State>, envelopes: Vec<Envelope<_>>| {
            hash(&ActorSystemSnapshot {
                actor_states: states.iter().map(|s| Rc::new(s)).collect::<Vec<_>>(),
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
        let mut checker = system.checker(KeepPaths::Yes, invariant);
        checker.check(1_000);
        assert_eq!(checker.visited.len(), 14);
        assert_eq!(checker.visited, FxHashSet::from_iter(vec![
            // When the network loses no messages...
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(0)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),

            // When the network loses the message for pinger-ponger state (0, 0)...
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(0)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (0, 1)
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::PingerState(0), State::PongerState(1)],
                Vec::new()),

            // When the network loses a message for pinger-ponger state (1, 1)
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(1) },
                ]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![
                    Envelope { src: 0, dst: 1, msg: Msg::Ping(0) },
                    Envelope { src: 1, dst: 0, msg: Msg::Pong(0) },
                ]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(1) }]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![Envelope { src: 1, dst: 0, msg: Msg::Pong(0) }]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
                vec![Envelope { src: 0, dst: 1, msg: Msg::Ping(0) }]),
            snapshot_hash(
                vec![State::PingerState(1), State::PongerState(1)],
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
        let mut checker = sys.checker(KeepPaths::No, invariant);
        let result = checker.check(1_000_000);
        assert_eq!(result, CheckResult::Pass);
        assert_eq!(checker.visited.len(), 4094);
    }
}
}

