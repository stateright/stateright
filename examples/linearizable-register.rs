//! Provides a linearizable register "shared memory" abstraction that can serve requests as long as
//! a quorum of actors is available  (e.g. 3 of 5). This code is based on the algorithm described
//! in "[Sharing Memory Robustly in Message-Passing
//! Systems](https://doi.org/10.1145/200836.200869)" by Attiya, Bar-Noy, and Dolev. "ABD" in the
//! types refers to the author names.
//!
//! For a succinct overview of the algorithm, I recommend:
//! http://muratbuffalo.blogspot.com/2012/05/replicatedfault-tolerant-atomic-storage.html

use serde_derive::{Deserialize, Serialize};
use stateright::Model;
use stateright::actor::{Actor, Id, majority, Out};
use stateright::actor::register::{TestRequestId, TestValue, RegisterMsg, RegisterMsg::*, RegisterTestSystem};
use stateright::actor::system::{model_peers, System, SystemState};
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

    fn on_start(&self, _id: Id, o: &mut Out<Self>) {
        o.set_state(Default::default());
    }

    fn on_msg(&self, id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Put(req_id, val) => {
                if state.phase.is_some() { return }
                o.broadcast(&self.peers, &Internal(Query(req_id)));

                let mut responses = HashableHashMap::default();
                responses.insert(id, (state.seq, state.val.clone()));

                let mut state = o.set_state(state.clone());
                state.phase = Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: src,
                    write: Some(val),
                    responses,
                });
            }
            Get(req_id) => {
                if state.phase.is_some() { return }
                o.broadcast(&self.peers, &Internal(Query(req_id)));

                let mut responses = HashableHashMap::default();
                responses.insert(id, (state.seq, state.val.clone()));

                let mut state = o.set_state(state.clone());
                state.phase = Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: src,
                    write: None,
                    responses,
                });
            }
            Internal(Query(req_id)) => {
                o.send(src, Internal(AckQuery(req_id, state.seq, state.val.clone())));
            }
            Internal(AckQuery(expected_req_id, seq, val)) => {
                if let Some(AbdPhase::Phase1 { request_id: req_id, .. }) = &state.phase {
                    if *req_id != expected_req_id { return }
                }
                let mut state = state.clone();
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
                    o.set_state(state);
                }
            }
            Internal(Record(req_id, seq, val)) => {
                o.send(src, Internal(AckRecord(req_id)));
                if seq > state.seq {
                    let mut state = o.set_state(state.clone());
                    state.seq = seq;
                    state.val = val;
                }
            }
            Internal(AckRecord(expected_req_id)) => {
                if let Some(AbdPhase::Phase2 { request_id: req_id, .. }) = &state.phase {
                    if *req_id != expected_req_id { return }
                }
                let mut state = state.clone();
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
                o.set_state(state);
            }
            // The following are ignored as they are actor system outputs.
            PutOk(_) | GetOk(_, _) => {},
        }
    }
}

fn within_boundary(state: &SystemState<RegisterTestSystem<AbdActor, AbdMsg>>) -> bool {
    state.actor_states.iter().all(|s| {
        s.seq.0 < 3
    })
}

#[cfg(test)]
#[test]
fn can_model_linearizable_register() {
    use stateright::actor::system::SystemAction::Deliver;
    let mut checker = RegisterTestSystem {
        servers: vec![
            AbdActor { peers: model_peers(0, 2) },
            AbdActor { peers: model_peers(1, 2) },
        ],
        put_count: 2,
        get_count: 1,
        within_boundary,
        .. Default::default()
    }.into_model().checker();
    checker.check(10_000);
    // FIXME: the model checker finds a linearizability violation because our linearizability check
    //        is too strict.
    assert_eq!(checker.assert_counterexample("linearizable").into_actions(), vec![
        Deliver { src: Id::from(998), dst: Id::from(1), msg: Put(1, 'B') },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Query(1)) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckQuery(1, (0, Id::from(0)), '\u{0}')) },
		Deliver { src: Id::from(997), dst: Id::from(0), msg: Get(2) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Query(2)) },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckQuery(2, (1, Id::from(1)), 'B')) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Record(2, (1, Id::from(1)), 'B')) },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckRecord(2)) }
    ]);
    assert_eq!(checker.assert_example("value chosen").into_actions(), vec![
        Deliver { src: Id::from(998), dst: Id::from(1), msg: Put(1, 'B') },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Query(1)) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(AckQuery(1, (0, Id::from(0)), '\u{0}')) },
		Deliver { src: Id::from(997), dst: Id::from(0), msg: Get(2) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Query(2)) },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckQuery(2, (1, Id::from(1)), 'B')) },
		Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Record(2, (1, Id::from(1)), 'B')) },
		Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(AckRecord(2)) }
    ]);
    assert_eq!(checker.generated_count(), 15_869);
}

fn main() {
    use clap::{App, AppSettings, Arg, SubCommand, value_t};
    use stateright::actor::spawn::spawn;
    use stateright::explorer::Explorer;
    use std::net::{SocketAddrV4, Ipv4Addr};

    env_logger::init_from_env(env_logger::Env::default().default_filter_or("debug"));

    let mut app = App::new("wor")
        .about("linearizable register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("put_count")
                .help("number of puts")
                .default_value("2"))
            .arg(Arg::with_name("get_count")
                .help("number of gets")
                .default_value("2")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("put_count")
                .help("number of puts")
                .default_value("2"))
            .arg(Arg::with_name("get_count")
                .help("number of gets")
                .default_value("2"))
            .arg(Arg::with_name("address")
                .help("address Explorer service should listen upon")
                .default_value("localhost:3000")))
        .subcommand(SubCommand::with_name("spawn")
            .about("spawn with messaging over UDP"));
    let args = app.clone().get_matches();

    match args.subcommand() {
        ("check", Some(args)) => {
            let put_count = std::cmp::min(
                26, value_t!(args, "put_count", u8).expect("put count missing"));
            let get_count = std::cmp::min(
                26, value_t!(args, "get_count", u8).expect("get count missing"));
            println!("Model checking a linearizable register with {} puts and {} gets.",
                     put_count, get_count);
            RegisterTestSystem {
                servers: vec![
                    AbdActor { peers: model_peers(0, 2) },
                    AbdActor { peers: model_peers(1, 2) },
                ],
                put_count: 2,
                get_count: 1,
                within_boundary,
                .. Default::default()
            }.into_model()
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let put_count = std::cmp::min(
                26, value_t!(args, "put_count", u8).expect("put count missing"));
            let get_count = std::cmp::min(
                26, value_t!(args, "get_count", u8).expect("get count missing"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for linearizable register with {} puts and {} gets on {}.",
                put_count, get_count, address);
            RegisterTestSystem {
                servers: vec![
                    AbdActor { peers: model_peers(0, 2) },
                    AbdActor { peers: model_peers(1, 2) },
                ],
                put_count: 2,
                get_count: 1,
                within_boundary,
                .. Default::default()
            }.into_model().checker().serve(address).unwrap();
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
            let handles = spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (id0, AbdActor { peers: vec![id1, id2] }),
                    (id1, AbdActor { peers: vec![id0, id2] }),
                    (id2, AbdActor { peers: vec![id0, id1] }),
                ]);
            for h in handles { let _ = h.join(); }
        }
        _ => app.print_help().unwrap(),
    }
}
