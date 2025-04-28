//! Provides a linearizable register "shared memory" abstraction that can serve requests as long as
//! a quorum of actors is available  (e.g. 3 of 5). This code is based on the algorithm described
//! in "[Sharing Memory Robustly in Message-Passing
//! Systems](https://doi.org/10.1145/200836.200869)" by Attiya, Bar-Noy, and Dolev. "ABD" in the
//! types refers to the author names.
//!
//! For a succinct overview of the algorithm, I recommend:
//! http://muratbuffalo.blogspot.com/2012/05/replicatedfault-tolerant-atomic-storage.html

use serde::{Deserialize, Serialize};
use stateright::actor::register::{RegisterActor, RegisterMsg, RegisterMsg::*};
use stateright::actor::{majority, model_peers, Actor, ActorModel, Id, Network, Out};
use stateright::report::WriteReporter;
use stateright::semantics::register::Register;
use stateright::semantics::LinearizabilityTester;
use stateright::util::{HashableHashMap, HashableHashSet};
use stateright::{Checker, Expectation, Model};
use std::borrow::Cow;
use std::fmt::Debug;
use std::hash::Hash;

type LogicalClock = u64;
type RequestId = u64;
type Seq = (LogicalClock, Id);
type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum AbdMsg {
    Query(RequestId),
    AckQuery(RequestId, Seq, Value),
    Record(RequestId, Seq, Value),
    AckRecord(RequestId),
}
use AbdMsg::*;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AbdState {
    seq: Seq,
    val: Value,
    phase: Option<AbdPhase>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AbdPhase {
    Phase1 {
        request_id: RequestId,
        requester_id: Id,
        write: Option<Value>,
        responses: HashableHashMap<Id, (Seq, Value)>,
    },
    Phase2 {
        request_id: RequestId,
        requester_id: Id,
        read: Option<Value>,
        acks: HashableHashSet<Id>,
    },
}

#[derive(Clone)]
pub struct AbdActor {
    pub(crate) peers: Vec<Id>,
}

impl Actor for AbdActor {
    type Msg = RegisterMsg<RequestId, Value, AbdMsg>;
    type State = AbdState;
    type Timer = ();
    type Random = ();
    type Storage = ();

    fn on_start(&self, id: Id, _storage: &Option<Self::Storage>, _o: &mut Out<Self>) -> Self::State {
        AbdState {
            seq: (0, id),
            val: Value::default(),
            phase: None,
        }
    }

    fn on_msg(
        &self,
        id: Id,
        state: &mut Cow<Self::State>,
        src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        match msg {
            Put(req_id, val) if state.phase.is_none() => {
                o.broadcast(&self.peers, &Internal(Query(req_id)));
                state.to_mut().phase = Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: src,
                    write: Some(val),
                    responses: {
                        let mut responses = HashableHashMap::default();
                        responses.insert(id, (state.seq, state.val));
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
                        responses.insert(id, (state.seq, state.val));
                        responses
                    },
                });
            }
            Internal(Query(req_id)) => {
                o.send(src, Internal(AckQuery(req_id, state.seq, state.val)));
            }
            Internal(AckQuery(expected_req_id, seq, val))
                if matches!(state.phase,
                            Some(AbdPhase::Phase1 { request_id, .. })
                            if request_id == expected_req_id) =>
            {
                let state = state.to_mut();
                if let Some(AbdPhase::Phase1 {
                    request_id: req_id,
                    requester_id: requester,
                    write,
                    responses,
                    ..
                }) = &mut state.phase
                {
                    responses.insert(src, (seq, val));
                    if responses.len() == majority(self.peers.len() + 1) {
                        // Quorum reached. Move to phase 2.

                        // Determine sequencer and value.
                        let (seq, val) = responses
                            .values()
                            // The following relies on the fact that sequencers are distinct.
                            // Otherwise the chosen response can vary even when given the same
                            // inputs due to the underlying `HashMap`'s random seed.
                            .max_by_key(|(seq, _)| seq)
                            .unwrap();
                        let mut seq = *seq;
                        let mut read = None;
                        let val = if let Some(val) = std::mem::take(write) {
                            seq = (seq.0 + 1, id);
                            val
                        } else {
                            read = Some(*val);
                            *val
                        };

                        // A future optimization could skip the recording phase if the replicas
                        // agree.
                        o.broadcast(&self.peers, &Internal(Record(*req_id, seq, val)));

                        // Self-send `Record`.
                        if seq > state.seq {
                            state.seq = seq;
                            state.val = val;
                        }

                        // Self-send `AckRecord`.
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
                    let state = state.to_mut();
                    state.seq = seq;
                    state.val = val;
                }
            }
            Internal(AckRecord(expected_req_id))
                if matches!(state.phase,
                            Some(AbdPhase::Phase2 { request_id, ref acks, .. })
                            if request_id == expected_req_id && !acks.contains(&src)) =>
            {
                let state = state.to_mut();
                if let Some(AbdPhase::Phase2 {
                    request_id: req_id,
                    requester_id: requester,
                    read,
                    acks,
                    ..
                }) = &mut state.phase
                {
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
    network: Network<<AbdActor as Actor>::Msg>,
}

impl AbdModelCfg {
    fn into_model(
        self,
    ) -> ActorModel<RegisterActor<AbdActor>, Self, LinearizabilityTester<Id, Register<Value>>> {
        ActorModel::new(
            self.clone(),
            LinearizabilityTester::new(Register(Value::default())),
        )
        .actors((0..self.server_count).map(|i| {
            RegisterActor::Server(AbdActor {
                peers: model_peers(i, self.server_count),
            })
        }))
        .actors((0..self.client_count).map(|_| RegisterActor::Client {
            put_count: 1,
            server_count: self.server_count,
        }))
        .init_network(self.network)
        .property(Expectation::Always, "linearizable", |_, state| {
            state.history.serialized_history().is_some()
        })
        .property(Expectation::Sometimes, "value chosen", |_, state| {
            for env in state.network.iter_deliverable() {
                if let RegisterMsg::GetOk(_req_id, value) = env.msg {
                    if *value != Value::default() {
                        return true;
                    }
                }
            }
            false
        })
        .record_msg_in(RegisterMsg::record_returns)
        .record_msg_out(RegisterMsg::record_invocations)
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
        network: Network::new_unordered_nonduplicating([]),
    }
    .into_model()
    .checker()
    .spawn_bfs()
    .join();
    checker.assert_properties();
    #[rustfmt::skip]
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
    assert_eq!(checker.unique_state_count(), 544);

    // DFS
    let checker = AbdModelCfg {
        client_count: 2,
        server_count: 2,
        network: Network::new_unordered_nonduplicating([]),
    }
    .into_model()
    .checker()
    .spawn_dfs()
    .join();
    checker.assert_properties();
    #[rustfmt::skip]
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
    assert_eq!(checker.unique_state_count(), 544);
}

fn main() -> Result<(), pico_args::Error> {
    use stateright::actor::spawn;
    use std::net::{Ipv4Addr, SocketAddrV4};

    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Model checking a linearizable register with {} clients.",
                client_count
            );
            AbdModelCfg {
                client_count,
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .spawn_dfs()
            .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Exploring state space for linearizable register with {} clients on {}.",
                client_count, address
            );
            AbdModelCfg {
                client_count,
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .serve(address);
        }
        Some("spawn") => {
            let port = 3000;

            println!("  A server that implements a linearizable register.");
            println!("  You can monitor and interact using tcpdump and netcat.");
            println!("  Use `tcpdump -D` if you see error `lo0: No such device exists`.");
            println!("Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u localhost {}", port);
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Put::<RequestId, Value, ()>(1, 'X')).unwrap()
            );
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Get::<RequestId, Value, ()>(2)).unwrap()
            );
            println!();

            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (
                        id0,
                        AbdActor {
                            peers: vec![id1, id2],
                        },
                    ),
                    (
                        id1,
                        AbdActor {
                            peers: vec![id0, id2],
                        },
                    ),
                    (
                        id2,
                        AbdActor {
                            peers: vec![id0, id1],
                        },
                    ),
                ],
            )
            .unwrap();
        }
        _ => {
            println!("USAGE:");
            println!("  ./linearizable-register check [CLIENT_COUNT] [NETWORK]");
            println!("  ./linearizable-register explore [CLIENT_COUNT] [ADDRESS] [NETWORK]");
            println!("  ./linearizable-register spawn");
            println!(
                "NETWORK: {}",
                Network::<<AbdActor as Actor>::Msg>::names().join(" | ")
            );
        }
    }

    Ok(())
}
