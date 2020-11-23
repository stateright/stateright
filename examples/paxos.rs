//! A cluster that implements Single Decree Paxos.

use serde_derive::{Deserialize, Serialize};
use stateright::{Model, Checker};
use stateright::actor::{Actor, DuplicatingNetwork, Id, model_peers, Out, System, SystemState};
use stateright::actor::register::{RegisterActorState, RegisterMsg, RegisterMsg::*, RegisterTestSystem, TestRequestId, TestValue};
use stateright::util::{HashableHashMap, HashableHashSet};

type Round = u32;
type Rank = u32;
type Ballot = (Round, Rank);
type Proposal = (TestRequestId, Id, TestValue);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
enum PaxosMsg {
    Prepare { ballot: Ballot },
    Prepared { ballot: Ballot, last_accepted: Option<(Ballot, Proposal)> },

    Accept { ballot: Ballot, proposal: Proposal },
    Accepted { ballot: Ballot },

    Decided { ballot: Ballot, proposal: Proposal },
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
struct PaxosActor { rank: Rank, peer_ids: Vec<Id> }

impl Actor for PaxosActor {
    type Msg = RegisterMsg<TestRequestId, TestValue, PaxosMsg>;
    type State = PaxosState;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) {
        o.set_state(PaxosState {
            // shared state
            ballot: (0, 0),

            // leader state
            proposal: None,
            prepares: Default::default(),
            accepts: Default::default(),

            // acceptor state
            accepted: None,
            is_decided: false,
        });
    }

    fn on_msg(&self, _id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Put(request_id, value) if !state.is_decided && state.proposal.is_none()  => {
                let mut state = state.clone();
                state.ballot = (state.ballot.0 + 1, self.rank);
                state.proposal = Some((request_id, src, value));
                state.prepares = Default::default();
                state.accepts = Default::default();
                o.broadcast(
                    &self.peer_ids,
                    &Internal(Prepare { ballot: state.ballot }));
                o.set_state(state);
            }
            Get(request_id) if state.is_decided => {
                if let Some((_ballot, (_request_id, _requester_id, value))) = state.accepted {
                    o.send(src, GetOk(request_id, value));
                } else {
                    // See `Internal(Decided ...)` case below.
                    unreachable!("accepted state present when decided");
                }
                // While it's tempting to `o.send(src, GetOk(request_id, None))` for undecided,
                // we don't know if a value was decided elsewhere and the delivery is pending. Our
                // solution is to not reply in this case, but a more useful choice might be
                // to broadcast to the other actors and let them reply to the originator, or query
                // the other actors and reply based on that.
            },
            Internal(Prepare { ballot }) if state.ballot < ballot => {
                let mut state = state.clone();
                state.ballot = ballot;
                o.send(src, Internal(Prepared {
                    ballot,
                    last_accepted: state.accepted,
                }));
                o.set_state(state);
            }
            Internal(Prepared { ballot, last_accepted })
            if ballot == state.ballot && !state.is_decided => {
                let mut state = state.clone();
                state.prepares.insert(src, last_accepted);
                if state.prepares.len() > (self.peer_ids.len() + 1) / 2 {
                    let proposal = state.prepares
                        .values().max().unwrap().map(|(_b, p)| p)
                        .unwrap_or_else(||
                            state.proposal.expect("proposal expected")); // See `Put` case above.
                    state.proposal = Some(proposal);
                    state.accepted = Some((ballot, proposal));
                    o.broadcast(&self.peer_ids, &Internal(Accept {
                        ballot,
                        proposal,
                    }));
                }
                o.set_state(state);
            }
            Internal(Accept { ballot, proposal })
            if state.ballot <= ballot && !state.is_decided => {
                let mut state = state.clone();
                state.ballot = ballot;
                state.accepted = Some((ballot, proposal));
                o.set_state(state);
                o.send(src, Internal(Accepted { ballot }));
            }
            Internal(Accepted { ballot }) if ballot == state.ballot => {
                let mut state = state.clone();
                state.accepts.insert(src);
                if state.accepts.len() > (self.peer_ids.len() + 1) / 2 {
                    state.is_decided = true;
                    let proposal = state.proposal
                        .expect("proposal expected"); // See `Put` case above.
                    o.broadcast(&self.peer_ids, &Internal(Decided {
                        ballot,
                        proposal,
                    }));
                    let (request_id, requester_id, _) = proposal;
                    o.send(requester_id, PutOk(request_id));
                }
                o.set_state(state);
            }
            Internal(Decided { ballot, proposal }) => {
                let mut state = state.clone();
                state.ballot = ballot;
                state.accepted = Some((ballot, proposal));
                state.is_decided = true;
                o.set_state(state);
            }
            _ => {}
        }
    }
}

fn within_boundary(state: &SystemState<RegisterTestSystem<PaxosActor, PaxosMsg>>) -> bool {
    state.actor_states.iter().all(|s| {
        if let RegisterActorState::Server(s) = &**s {
            s.ballot.0 < 4
        } else {
            true
        }
    })
}

#[cfg(test)]
#[test]
fn can_model_paxos() {
    use stateright::actor::SystemAction::Deliver;

    // BFS
    let checker = RegisterTestSystem {
        servers: vec![
            PaxosActor { rank: 0, peer_ids: model_peers(0, 3) },
            PaxosActor { rank: 1, peer_ids: model_peers(1, 3) },
            PaxosActor { rank: 2, peer_ids: model_peers(2, 3) },
        ],
        client_count: 2,
        within_boundary,
        duplicating_network: DuplicatingNetwork::No,
        .. Default::default()
    }.into_model().checker().spawn_bfs().join();
    checker.assert_properties();
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(4), dst: Id::from(1), msg: Put(4, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Accept { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Accept { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(1), dst: Id::from(4), msg: PutOk(4) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Decided { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(4), dst: Id::from(2), msg: Get(8) },
     ]);
    assert_eq!(checker.generated_count(), 1_747);

    // DFS
    let checker = RegisterTestSystem {
        servers: vec![
            PaxosActor { rank: 0, peer_ids: model_peers(0, 3) },
            PaxosActor { rank: 1, peer_ids: model_peers(1, 3) },
            PaxosActor { rank: 2, peer_ids: model_peers(2, 3) },
        ],
        client_count: 2,
        within_boundary,
        duplicating_network: DuplicatingNetwork::No,
        .. Default::default()
    }.into_model().checker().spawn_dfs().join();
    checker.assert_properties();
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(4), dst: Id::from(1), msg: Put(4, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Accept { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Accept { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(1), dst: Id::from(4), msg: PutOk(4) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Decided { ballot: (1, 1), proposal: (4, Id::from(4), 'B') }) },
        Deliver { src: Id::from(4), dst: Id::from(2), msg: Get(8) },
     ]);
    assert_eq!(checker.generated_count(), 1_747);
}

fn main() {
    use clap::{App, AppSettings, Arg, SubCommand, value_t};
    use stateright::actor::spawn;
    use std::net::{SocketAddrV4, Ipv4Addr};

    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut app = App::new("paxos")
        .about("single decree paxos")
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
            println!("Model checking Single Decree Paxos with {} clients.",
                     client_count);
            RegisterTestSystem {
                servers: vec![
                    PaxosActor { rank: 0, peer_ids: model_peers(0, 3) },
                    PaxosActor { rank: 1, peer_ids: model_peers(1, 3) },
                    PaxosActor { rank: 2, peer_ids: model_peers(2, 3) },
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
                "Exploring state space for Single Decree Paxos with {} clients on {}.",
                client_count, address);
            RegisterTestSystem {
                servers: vec![
                    PaxosActor { rank: 0, peer_ids: model_peers(0, 3) },
                    PaxosActor { rank: 1, peer_ids: model_peers(1, 3) },
                    PaxosActor { rank: 2, peer_ids: model_peers(2, 3) },
                ],
                client_count,
                within_boundary,
                duplicating_network: DuplicatingNetwork::No,
                .. Default::default()
            }.into_model().checker()
                .threads(num_cpus::get()).spawn_bfs()
                .serve(address);
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A set of servers that implement Single Decree Paxos.");
            println!("  You can monitor and interact using tcpdump and netcat. Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u localhost {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<TestRequestId, TestValue, ()>(1, 'X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<TestRequestId, TestValue, ()>(2)).unwrap());
            println!();

            // WARNING: Omits `ordered_reliable_link` to keep the message
            //          protocol simple for `nc`.
            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 0));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            let handles = spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (id0, PaxosActor { rank: 0, peer_ids: vec![id1, id2] }),
                    (id1, PaxosActor { rank: 1, peer_ids: vec![id0, id2] }),
                    (id2, PaxosActor { rank: 2, peer_ids: vec![id0, id1] }),
                ]);
            for h in handles { let _ = h.join(); }
        }
        _ => app.print_help().unwrap(),
    }
}
