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
use stateright::{Checker, Model};
use stateright::actor::{Actor, DuplicatingNetwork, Id, majority, model_peers, Out, System, SystemState};
use stateright::actor::register::{RegisterActorState, RegisterMsg, RegisterMsg::*, RegisterTestSystem, TestRequestId, TestValue};
use stateright::util::{HashableHashMap, HashableHashSet};
use std::fmt::Debug;
use std::hash::Hash;

type WriteCount = u64;
type Seq = (WriteCount, Id);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(Serialize, Deserialize)]
pub enum AbdMsg {
    Query(TestRequestId),
    AckQuery(TestRequestId, Seq, TestValue),
    Record(TestRequestId, Seq, TestValue),
    AckRecord(TestRequestId),
}
use AbdMsg::*;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct AbdState {
    seq: Seq,
    val: TestValue,
    phase: Option<AbdPhase>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AbdPhase {
    Phase1 { request_id: TestRequestId, requester_id: Id, write: Option<TestValue>, responses: HashableHashMap<Id, (Seq, TestValue)> },
    Phase2 { request_id: TestRequestId, requester_id: Id, read: Option<TestValue>, acks: HashableHashSet<Id> },
}

#[derive(Clone)]
pub struct AbdActor {
    pub(crate) peers: Vec<Id>,
}

impl Actor for AbdActor {
    type Msg = RegisterMsg<TestRequestId, TestValue, AbdMsg>;
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

fn within_boundary(state: &SystemState<RegisterTestSystem<AbdActor, AbdMsg>>) -> bool {
    state.actor_states.iter().all(|s| {
        if let RegisterActorState::Server(s) = &**s {
            s.seq.0 <= 3
        } else {
            true
        }
    })
}

#[cfg(test)]
#[test]
fn can_model_linearizable_register() {
    use stateright::actor::SystemAction::Deliver;

    // BFS
    let checker = RegisterTestSystem {
        servers: vec![
            AbdActor { peers: model_peers(0, 2) },
            AbdActor { peers: model_peers(1, 2) },
        ],
        client_count: 2,
        within_boundary,
        duplicating_network: DuplicatingNetwork::No,
        .. Default::default()
    }.into_model().checker().spawn_bfs().join();
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
    let checker = RegisterTestSystem {
        servers: vec![
            AbdActor { peers: model_peers(0, 2) },
            AbdActor { peers: model_peers(1, 2) },
        ],
        client_count: 2,
        within_boundary,
        duplicating_network: DuplicatingNetwork::No,
        .. Default::default()
    }.into_model().checker().spawn_dfs().join();
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
            RegisterTestSystem {
                servers: vec![
                    AbdActor { peers: model_peers(0, 2) },
                    AbdActor { peers: model_peers(1, 2) },
                ],
                client_count,
                within_boundary,
                duplicating_network: DuplicatingNetwork::No,
                .. Default::default()
            }.into_model().checker()
                .threads(num_cpus::get()).spawn_dfs()
                .report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client count missing"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for linearizable register with {} clients on {}.",
                 client_count, address);
            RegisterTestSystem {
                servers: vec![
                    AbdActor { peers: model_peers(0, 2) },
                    AbdActor { peers: model_peers(1, 2) },
                ],
                client_count,
                within_boundary,
                duplicating_network: DuplicatingNetwork::No,
                .. Default::default()
            }.into_model().checker()
                .threads(num_cpus::get())
                .serve(address);
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a linearizable register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u localhost {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<TestRequestId, TestValue, ()>(1, 'X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<TestRequestId, TestValue, ()>(2)).unwrap());
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
