//! This is an implementation of Single Decree Paxos, an algorithm that ensures a cluster of
//! servers never disagrees on a value.
//!
//! # The Algorithm
//!
//! The Paxos algorithm is composed of two phases. These are best understood in reverse order.
//!
//! ## Phase 2
//!
//! Phase 2 involves broadcasting a proposal (or a sequence of proposals in the case of
//! Multipaxos). If a quorum accepts a proposal, it is considered "decided" by the cluster even if
//! the leader does not observe that decision, e.g. due to message loss.
//!
//! ## Phase 1
//!
//! Phase 1 solves the more complex problem of leadership handoff by introducing a notion of
//! leadership "terms" and a technique for ensuring new terms are consistent with earlier terms.
//!
//! 1. Each term has a distinct leader. Before proposing values during its term, the
//!    leader broadcasts a message that closes previous terms. Once a quorum replies, the leader
//!    knows that previous leaders are unable to reach new (and possibly contradictory) decisions.
//! 2. The leader also needs to learn the proposal that was decided by previous terms (or sequence
//!    of proposals for Multipaxos), so in their replies, the servers indicate their previously
//!    accepted proposals.
//! 3. The leader cannot be guaranteed to know if a proposal was decided unless it talks with every
//!    server, which undermines the availability of the system, so Paxos leverages a clever trick:
//!    the leader drives the most recent accepted proposal to a quorum (and for Multipaxos it does
//!    this for each index in the sequence of proposals). It only needs to look at the most recent
//!    proposal because any previous leader would have done the same prior to sending its new
//!    proposals.
//! 4. Many optimizations are possible. For example, the leader can skip driving consensus on a
//!    previous proposal if the Phase 1 quorum already agrees or the leader observes auxiliary
//!    state from which it can infer agreement. The latter optimizations are particularly important
//!    for Multipaxos.
//!
//! ## Leadership Terms
//!
//! It is safe to start a new term at any time.
//!
//! In Multipaxos, a term is typically maintained until the leader times out (as observed by a
//! peer), allowing that leader to propose a sequence of values while (1) avoiding contention
//! and (2) only paying the cost of a single message round trip for each proposal.
//!
//! In contrast, with Single Decree Paxos, a term is typically coupled to the life of a client
//! request, so each client request gets a new term. This can result in contention if many values
//! are proposed in parallel, but the following implementation follows this approach to match how
//! the algorithm is typically described.

use serde::{Deserialize, Serialize};
use stateright::actor::register::{RegisterActor, RegisterMsg, RegisterMsg::*};
use stateright::actor::{majority, model_peers, Actor, ActorModel, Id, Network, Out};
use stateright::report::WriteReporter;
use stateright::semantics::register::Register;
use stateright::semantics::LinearizabilityTester;
use stateright::util::{HashableHashMap, HashableHashSet};
use stateright::{Checker, Expectation, Model, UniformChooser};
use std::borrow::Cow;
use std::time::Duration;

type Round = u32;
type Ballot = (Round, Id);
type Proposal = (RequestId, Id, Value);
type RequestId = u64;
type Value = char;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
enum PaxosMsg {
    Prepare {
        ballot: Ballot,
    },
    Prepared {
        ballot: Ballot,
        last_accepted: Option<(Ballot, Proposal)>,
    },

    Accept {
        ballot: Ballot,
        proposal: Proposal,
    },
    Accepted {
        ballot: Ballot,
    },

    Decided {
        ballot: Ballot,
        proposal: Proposal,
    },
}
use PaxosMsg::*;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PaxosState {
    // shared state
    ballot: Ballot,

    // leader state
    proposal: Option<Proposal>,
    prepares: HashableHashMap<Id, Option<(Ballot, Proposal)>>,
    accepts: HashableHashSet<Id>,

    // acceptor state
    accepted: Option<(Ballot, Proposal)>,
    is_decided: bool,
}

#[derive(Clone)]
struct PaxosActor {
    peer_ids: Vec<Id>,
}

impl Actor for PaxosActor {
    type Msg = RegisterMsg<RequestId, Value, PaxosMsg>;
    type State = PaxosState;
    type Timer = ();
    type Random = ();
    type Storage = ();

    fn name(&self) -> String {
        "Paxos Server".to_owned()
    }

    fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, _o: &mut Out<Self>) -> Self::State {
        PaxosState {
            // shared state
            ballot: (0, Id::from(0)),

            // leader state
            proposal: None,
            prepares: Default::default(),
            accepts: Default::default(),

            // acceptor state
            accepted: None,
            is_decided: false,
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
        if state.is_decided {
            if let Get(request_id) = msg {
                // While it's tempting to `o.send(src, GetOk(request_id, None))` for undecided,
                // we don't know if a value was decided elsewhere and the delivery is pending. Our
                // solution is to not reply in this case, but a more useful choice might be
                // to broadcast to the other actors and let them reply to the originator, or query
                // the other actors and reply based on that.
                let (_b, (_req_id, _src, value)) =
                    state.accepted.expect("decided but lacks accepted state");
                o.send(src, GetOk(request_id, value));
            };
            return;
        }

        match msg {
            Put(request_id, value) if state.proposal.is_none() => {
                let state = state.to_mut();
                state.proposal = Some((request_id, src, value));
                state.prepares = Default::default();
                state.accepts = Default::default();

                // Simulate `Prepare` self-send.
                state.ballot = (state.ballot.0 + 1, id);
                // Simulate `Prepared` self-send.
                state.prepares.insert(id, state.accepted);

                o.broadcast(
                    &self.peer_ids,
                    &Internal(Prepare {
                        ballot: state.ballot,
                    }),
                );
            }
            Internal(Prepare { ballot }) if state.ballot < ballot => {
                state.to_mut().ballot = ballot;
                o.send(
                    src,
                    Internal(Prepared {
                        ballot,
                        last_accepted: state.accepted,
                    }),
                );
            }
            Internal(Prepared {
                ballot,
                last_accepted,
            }) if ballot == state.ballot => {
                let state = state.to_mut();
                state.prepares.insert(src, last_accepted);
                if state.prepares.len() == majority(self.peer_ids.len() + 1) {
                    // This stage is best understood as "leadership handoff," in which this term's
                    // leader needs to ensure it does not contradict a decision (a quorum of
                    // accepts) from a previous term. Here's how:
                    //
                    // 1. To start this term, the leader first "locked" the older terms from
                    //    additional accepts via the `Prepare` messages.
                    // 2. If the servers reached a decision in a previous term, then the observed
                    //    prepare quorum is guaranteed to contain that accepted proposal, and we
                    //    have to favor that one.
                    // 3. We only have to drive the proposal accepted by the most recent term
                    //    because the leaders of the previous terms would have done the same before
                    //    asking their peers to accept proposals (so any proposals accepted by
                    //    earlier terms either match the most recently accepted proposal or are
                    //    guaranteed to have never reached quorum and so are safe to ignore).
                    // 4. If no proposals were previously accepted, the leader is safe to proceed
                    //    with the one from the client.
                    let proposal = state
                        .prepares
                        .values()
                        .max()
                        .unwrap()
                        .map(|(_b, p)| p)
                        .unwrap_or_else(|| state.proposal.expect("proposal expected")); // See `Put` case above.
                    state.proposal = Some(proposal);

                    // Simulate `Accept` self-send.
                    state.accepted = Some((ballot, proposal));
                    // Simulate `Accepted` self-send.
                    state.accepts.insert(id);

                    o.broadcast(&self.peer_ids, &Internal(Accept { ballot, proposal }));
                }
            }
            Internal(Accept { ballot, proposal }) if state.ballot <= ballot => {
                let state = state.to_mut();
                state.ballot = ballot;
                state.accepted = Some((ballot, proposal));
                o.send(src, Internal(Accepted { ballot }));
            }
            Internal(Accepted { ballot }) if ballot == state.ballot => {
                let state = state.to_mut();
                state.accepts.insert(src);
                if state.accepts.len() == majority(self.peer_ids.len() + 1) {
                    state.is_decided = true;
                    let proposal = state.proposal.expect("proposal expected"); // See `Put` case above.
                    o.broadcast(&self.peer_ids, &Internal(Decided { ballot, proposal }));
                    let (request_id, requester_id, _) = proposal;
                    o.send(requester_id, PutOk(request_id));
                }
            }
            Internal(Decided { ballot, proposal }) => {
                let state = state.to_mut();
                state.ballot = ballot;
                state.accepted = Some((ballot, proposal));
                state.is_decided = true;
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct PaxosModelCfg {
    client_count: usize,
    server_count: usize,
    network: Network<<PaxosActor as Actor>::Msg>,
}

impl PaxosModelCfg {
    fn into_model(
        self,
    ) -> ActorModel<RegisterActor<PaxosActor>, Self, LinearizabilityTester<Id, Register<Value>>>
    {
        ActorModel::new(
            self.clone(),
            LinearizabilityTester::new(Register(Value::default())),
        )
        .actors((0..self.server_count).map(|i| {
            RegisterActor::Server(PaxosActor {
                peer_ids: model_peers(i, self.server_count),
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
fn can_model_paxos() {
    use stateright::actor::ActorModelAction::Deliver;

    // BFS
    let checker = PaxosModelCfg {
        client_count: 2,
        server_count: 3,
        network: Network::new_unordered_nonduplicating([]),
    }
    .into_model()
    .checker()
    .spawn_bfs()
    .join();
    checker.assert_properties();
    #[rustfmt::skip]
    checker.assert_discovery("value chosen", vec![
        Deliver { src: 4.into(), dst: 1.into(), msg: Put(4, 'B') },
        Deliver { src: 1.into(), dst: 0.into(), msg: Internal(Prepare { ballot: (1, 1.into()) }) },
        Deliver { src: 0.into(), dst: 1.into(), msg: Internal(Prepared { ballot: (1, 1.into()), last_accepted: None }) },
        Deliver { src: 1.into(), dst: 2.into(), msg: Internal(Accept { ballot: (1, 1.into()), proposal: (4, 4.into(), 'B') }) },
        Deliver { src: 2.into(), dst: 1.into(), msg: Internal(Accepted { ballot: (1, 1.into()) }) },
        Deliver { src: 1.into(), dst: 4.into(), msg: PutOk(4) },
        Deliver { src: 1.into(), dst: 2.into(), msg: Internal(Decided { ballot: (1, 1.into()), proposal: (4, 4.into(), 'B') }) },
        Deliver { src: 4.into(), dst: 2.into(), msg: Get(8) }
    ]);
    assert_eq!(checker.unique_state_count(), 16_668);

    // DFS
    let checker = PaxosModelCfg {
        client_count: 2,
        server_count: 3,
        network: Network::new_unordered_nonduplicating([]),
    }
    .into_model()
    .checker()
    .spawn_dfs()
    .join();
    checker.assert_properties();
    #[rustfmt::skip]
    checker.assert_discovery("value chosen", vec![
        Deliver { src: 4.into(), dst: 1.into(), msg: Put(4, 'B') },
        Deliver { src: 1.into(), dst: 0.into(), msg: Internal(Prepare { ballot: (1, 1.into()) }) },
        Deliver { src: 0.into(), dst: 1.into(), msg: Internal(Prepared { ballot: (1, 1.into()), last_accepted: None }) },
        Deliver { src: 1.into(), dst: 2.into(), msg: Internal(Accept { ballot: (1, 1.into()), proposal: (4, 4.into(), 'B') }) },
        Deliver { src: 2.into(), dst: 1.into(), msg: Internal(Accepted { ballot: (1, 1.into()) }) },
        Deliver { src: 1.into(), dst: 4.into(), msg: PutOk(4) },
        Deliver { src: 1.into(), dst: 2.into(), msg: Internal(Decided { ballot: (1, 1.into()), proposal: (4, 4.into(), 'B') }) },
        Deliver { src: 4.into(), dst: 2.into(), msg: Get(8) }
    ]);
    assert_eq!(checker.unique_state_count(), 16_668);
}

fn main() -> Result<(), pico_args::Error> {
    use stateright::actor::spawn;
    use std::net::{Ipv4Addr, SocketAddrV4};

    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check-bfs") | Some("check") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Model checking Single Decree Paxos with {} clients.",
                client_count
            );
            PaxosModelCfg {
                client_count,
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .spawn_bfs()
            .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("check-dfs") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Model checking Single Decree Paxos with {} clients.",
                client_count
            );
            PaxosModelCfg {
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
        Some("check-simulation") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Model checking Single Decree Paxos with {} clients.",
                client_count
            );
            PaxosModelCfg {
                client_count,
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .timeout(Duration::from_secs(10))
            .spawn_simulation(0, UniformChooser)
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
                "Exploring state space for Single Decree Paxos with {} clients on {}.",
                client_count, address
            );
            PaxosModelCfg {
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

            println!("  A set of servers that implement Single Decree Paxos.");
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

            // WARNING: Omits `ordered_reliable_link` to keep the message
            //          protocol simple for `nc`.
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
                        PaxosActor {
                            peer_ids: vec![id1, id2],
                        },
                    ),
                    (
                        id1,
                        PaxosActor {
                            peer_ids: vec![id0, id2],
                        },
                    ),
                    (
                        id2,
                        PaxosActor {
                            peer_ids: vec![id0, id1],
                        },
                    ),
                ],
            )
            .unwrap();
        }
        _ => {
            println!("USAGE:");
            println!("  ./paxos check-dfs [CLIENT_COUNT] [NETWORK]");
            println!("  ./paxos check-bfs [CLIENT_COUNT] [NETWORK]");
            println!("  ./paxos check-simulation [CLIENT_COUNT] [NETWORK]");
            println!("  ./paxos explore [CLIENT_COUNT] [ADDRESS] [NETWORK]");
            println!("  ./paxos spawn");
            println!(
                "NETWORK: {}",
                Network::<<PaxosActor as Actor>::Msg>::names().join(" | ")
            );
        }
    }

    Ok(())
}
