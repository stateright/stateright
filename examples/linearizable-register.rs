//! Provides a linearizable register "shared memory" abstraction that can serve requests as long as
//! a quorum of actors is available  (e.g. 3 of 5). This code is based on the algorithm described
//! in "[Sharing Memory Robustly in Message-Passing
//! Systems](https://doi.org/10.1145/200836.200869)" by Attiya, Bar-Noy, and Dolev. "ABD" in the
//! types refers to the author names.
//!
//! For a succinct overview of the algorithm, I recommend:
//! http://muratbuffalo.blogspot.com/2012/05/replicatedfault-tolerant-atomic-storage.html

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use stateright::{Checker, Expectation, Model};
use stateright::actor::{
    Actor, ActorModel, DuplicatingNetwork, Id, majority, model_peers, Out};
use stateright::actor::register::{
    RegisterActor, RegisterActorState, RegisterMsg, RegisterMsg::*};
use stateright::semantics::LinearizabilityTester;
use stateright::semantics::register::Register;
use stateright::util::{HashableHashMap, HashableHashSet};
use std::fmt::Debug;
use std::hash::Hash;

type LogicalClock = u64;
type RequestId = u64;
type Seq = (LogicalClock, Id);
type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(Serialize, Deserialize)]
pub enum AbdMsg {
    Query(RequestId),
    AckQuery(RequestId, Seq, Value),
    Record(RequestId, Seq, Value),
    AckRecord(RequestId),
}
use AbdMsg::*;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct AbdState {
    seq: Seq,
    val: Value,
    phase: Option<AbdPhase>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AbdPhase {
    Phase1 { request_id: RequestId, requester_id: Id, write: Option<Value>, responses: HashableHashMap<Id, (Seq, Value)> },
    Phase2 { request_id: RequestId, requester_id: Id, read: Option<Value>, acks: HashableHashSet<Id> },
}

#[derive(Clone)]
pub struct AbdActor {
    pub(crate) peers: Vec<Id>,
}

impl Actor for AbdActor {
    type Msg = RegisterMsg<RequestId, Value, AbdMsg>;
    type State = AbdState;

    fn on_start(&self, _id: Id, _o: &mut Out<Self>) -> Self::State {
        Default::default()
    }

    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Put(req_id, val) if state.phase.is_none() => {
                o.broadcast(&self.peers, &Internal(Query(req_id)));
                state.to_mut().phase = Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: src,
                    write: Some(val),
                    responses: {
                        let mut responses = HashableHashMap::default();
                        responses.insert(id, (state.seq, state.val.clone()));
                        responses
                    },
                });
            }
            Get(req_id) if state.phase.is_none() => {
                o.broadcast(&self.peers, &Internal(Query(req_id)));
                state.to_mut().phase = Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: src,
                    write: None,
                    responses: {
                        let mut responses = HashableHashMap::default();
                        responses.insert(id, (state.seq, state.val.clone()));
                        responses
                    },
                });
            }
            Internal(Query(req_id)) => {
                o.send(src, Internal(AckQuery(req_id, state.seq, state.val.clone())));
            }
            Internal(AckQuery(expected_req_id, seq, val))
                if matches!(state.phase,
                            Some(AbdPhase::Phase1 { request_id, .. })
                            if request_id == expected_req_id) =>
            {
                let mut state = state.to_mut();
                if let Some(AbdPhase::Phase1 { request_id: req_id, requester_id: requester, write, responses, .. }) = &mut state.phase {
                    responses.insert(src, (seq, val));
                    if responses.len() == majority(self.peers.len() + 1) {
                        // Quorum reached. Move to phase 2.

                        // Determine sequencer and value.
                        let (_, (seq, val)) = responses.into_iter()
                            .max_by_key(|(_, (seq, _))| seq)
                            .unwrap();
                        let mut seq = *seq;
                        let mut read = None;
                        let val = if let Some(val) = std::mem::take(write) {
                            seq = (seq.0 + 1, id);
                            val
                        } else {
                            read = Some(val.clone());
                            val.clone()
                        };

                        // A future optimization could skip the recording phase if the replicas
                        // agree.
                        o.broadcast(&self.peers, &Internal(Record(*req_id, seq, val.clone())));

                        state.seq = seq;
                        state.val = val;

                        let mut acks = HashableHashSet::default();
                        acks.insert(id);

                        state.phase = Some(AbdPhase::Phase2 {
                            request_id: *req_id,
                            requester_id: std::mem::take(requester),
                            read,
                            acks,
                        });
                    }
                }
            }
            Internal(Record(req_id, seq, val)) => {
                o.send(src, Internal(AckRecord(req_id)));
                if seq > state.seq {
                    let mut state = state.to_mut();
                    state.seq = seq;
                    state.val = val;
                }
            }
            Internal(AckRecord(expected_req_id))
                if matches!(state.phase,
                            Some(AbdPhase::Phase2 { request_id, .. })
                            if request_id == expected_req_id) =>
            {
                let mut state = state.to_mut();
                if let Some(AbdPhase::Phase2 { request_id: req_id, requester_id: requester, read, acks, .. }) = &mut state.phase {
                    acks.insert(src);
                    if acks.len() == majority(self.peers.len() + 1) {
                        let msg = if let Some(val) = read {
                            GetOk(*req_id, std::mem::take(val))
                        } else {
                            PutOk(*req_id)
                        };
                        o.send(*requester, msg);
                        state.phase = None;
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct AbdModelCfg {
    client_count: usize,
    server_count: usize,
    max_clock: LogicalClock,
}

impl AbdModelCfg {
    fn into_model(self) ->
        ActorModel<
            RegisterActor<AbdActor>,
            Self,
            LinearizabilityTester<Id, Register<Value>>>
    {
        ActorModel::new(
                self.clone(),
                LinearizabilityTester::new(Register(Value::default()))
            )
            .actors((0..self.server_count)
                    .map(|i| RegisterActor::Server(AbdActor {
                        peers: model_peers(i, self.server_count),
                    })))
            .actors((0..self.client_count)
                    .map(|_| RegisterActor::Client {
                        server_count: self.server_count,
                    }))
            .duplicating_network(DuplicatingNetwork::No)
            .property(Expectation::Always, "linearizable", |_, state| {
                state.history.serialized_history().is_some()
            })
            .property(Expectation::Sometimes, "value chosen", |_, state| {
                for env in &state.network {
                    if let RegisterMsg::GetOk(_req_id, value) = env.msg {
                        if value != Value::default() { return true; }
                    }
                }
                false
            })
            .record_msg_in(RegisterMsg::record_returns)
            .record_msg_out(RegisterMsg::record_invocations)
            .within_boundary(|cfg, state| {
                state.actor_states.iter().all(|s| {
                    if let RegisterActorState::Server(s) = &**s {
                        s.seq.0 <= cfg.max_clock
                    } else {
                        true
                    }
                })
            })
    }
}

#[cfg(test)]
#[test]
fn can_model_linearizable_register() {
    use stateright::actor::ActorModelAction::Deliver;

    // BFS
    let checker = AbdModelCfg {
            client_count: 2,
            server_count: 2,
            max_clock: 3,
        }
        .into_model().checker().spawn_bfs().join();
    checker.assert_properties();
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(3), dst: Id::from(1), msg: Put(3, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Query(3)) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckQuery(3, (0, Id::from(0)), '\u{0}')) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Record(3, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckRecord(3)) },
        Deliver { src: Id::from(1), dst: Id::from(3), msg: PutOk(3) },
        Deliver { src: Id::from(3), dst: Id::from(0), msg: Get(6) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Query(6)) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckQuery(6, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Record(6, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckRecord(6)) },
    ]);
    assert_eq!(checker.generated_count(), 1321);

    // DFS
    let checker = AbdModelCfg {
            client_count: 2,
            server_count: 2,
            max_clock: 3,
        }
        .into_model().checker().spawn_dfs().join();
    checker.assert_properties();
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(3), dst: Id::from(1), msg: Put(3, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Query(3)) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckQuery(3, (0, Id::from(0)), '\u{0}')) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Record(3, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckRecord(3)) },
        Deliver { src: Id::from(1), dst: Id::from(3), msg: PutOk(3) },
        Deliver { src: Id::from(3), dst: Id::from(0), msg: Get(6) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Query(6)) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckQuery(6, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Record(6, (1, Id::from(1)), 'B')) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckRecord(6)) },
    ]);
    assert_eq!(checker.generated_count(), 1321);
}

fn main() {
    use clap::{App, AppSettings, Arg, SubCommand, value_t};
    use stateright::actor::spawn;
    use std::net::{SocketAddrV4, Ipv4Addr};

    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut app = App::new("wor")
        .about("linearizable register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("client_count")
                .help("number of clients")
                .default_value("2")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("client_count")
                .help("number of clients")
                .default_value("2"))
            .arg(Arg::with_name("address")
                .help("address Explorer service should listen upon")
                .default_value("localhost:3000")))
        .subcommand(SubCommand::with_name("spawn")
            .about("spawn with messaging over UDP"));
    let args = app.clone().get_matches();

    match args.subcommand() {
        ("check", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client count missing"));
            println!("Model checking a linearizable register with {} clients.",
                     client_count);
            AbdModelCfg {
                    client_count: client_count as usize,
                    server_count: 2,
                    max_clock: 3,
                }
                .into_model().checker().threads(num_cpus::get())
                .spawn_dfs().report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client count missing"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for linearizable register with {} clients on {}.",
                 client_count, address);
            AbdModelCfg {
                    client_count: client_count as usize,
                    server_count: 2,
                    max_clock: 3,
                }
                .into_model().checker().threads(num_cpus::get())
                .serve(address);
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a linearizable register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u localhost {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<RequestId, Value, ()>(1, 'X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<RequestId, Value, ()>(2)).unwrap());
            println!();

            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 0));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (id0, AbdActor { peers: vec![id1, id2] }),
                    (id1, AbdActor { peers: vec![id0, id2] }),
                    (id2, AbdActor { peers: vec![id0, id1] }),
                ]).unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}
