//! A cluster that implements Single Decree Paxos.

#[macro_use]
extern crate clap;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate stateright;

use clap::*;
use stateright::*;
use stateright::actor::*;
use stateright::actor::model::*;
use stateright::actor::register::*;
use stateright::actor::register::RegisterMsg::*;
use std::collections::*;

type Round = u32;
type Rank = u32;
type Ballot = (Round, Rank);
type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(Serialize, Deserialize)]
enum ServerMsg {
    Prepare { ballot: Ballot },
    Prepared { ballot: Ballot, last_accepted: Option<(Ballot, Value)> },

    Accept { ballot: Ballot, value: Value },
    Accepted { ballot: Ballot },

    Decided { ballot: Ballot, value: Value },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ServerState<Id> {
    // shared state
    ballot: Ballot,

    // leader state
    proposal: Option<Value>,
    prepares: BTreeMap<Id, Option<(Ballot, Value)>>,
    accepts: BTreeSet<Id>,

    // acceptor state
    accepted: Option<(Ballot, Value)>,
    is_decided: bool,
}

struct ServerCfg<Id> { rank: Rank, peer_ids: Vec<Id> }

impl<Id: Copy + Ord> Actor<Id> for ServerCfg<Id> {
    type Msg = RegisterMsg<Value, ServerMsg>;
    type State = ServerState<Id>;

    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
        ActorResult::new(ServerState {
            // shared state
            ballot: (0, 0),

            // leader state
            proposal: None,
            prepares: Default::default(),
            accepts: Default::default(),

            // acceptor state
            accepted: None,
            is_decided: false,
        })
    }

    fn advance(&self, input: ActorInput<Id, Self::Msg>, actor: &mut ActorResult<Id, Self::Msg, Self::State>) {
        use ServerMsg::*;

        let ActorInput::Deliver { src, msg } = input;
        let action = &mut actor.action;
        let state = &mut actor.state;
        let outputs = &mut actor.outputs;

        match msg {
            Put { value } => {
                if !state.is_decided {
                    // reduce state space until upcoming model checking optimizations land
                    if state.proposal.is_some() || state.ballot.0 == 3 { return }

                    *action = "Got Put. Leading new ballot by requesting Prepares.";
                    state.ballot = (state.ballot.0 + 1, self.rank);
                    state.proposal = Some(value);
                    state.prepares = Default::default();
                    state.accepts = Default::default();
                    outputs.broadcast(
                        &self.peer_ids,
                        &Internal(Prepare { ballot: state.ballot }));
                }
            }
            Get => {
                if let Some((_ballot, value)) = state.accepted {
                    if state.is_decided {
                        *action = "Responding to Get.";
                        outputs.send(src, Respond { value });
                    }
                }
            }
            Internal(Prepare { ballot }) => {
                if state.ballot < ballot {
                    *action = "Preparing.";
                    state.ballot = ballot;
                    outputs.send(src, Internal(Prepared {
                        ballot,
                        last_accepted: state.accepted,
                    }));
                }
            }
            Internal(Prepared { ballot, last_accepted }) => {
                if ballot == state.ballot {
                    *action = "Recording Prepared.";
                    state.prepares.insert(src, last_accepted);
                    if state.prepares.len() > (self.peer_ids.len() + 1)/2 {
                        *action = "Recording Prepared. Got quorum. Requesting Accepts.";
                        state.proposal = state.prepares
                            .values().max().unwrap().map(|(_b,v)| v)
                            .or(state.proposal);
                        state.accepted = Some((ballot, state.proposal.unwrap()));
                        outputs.broadcast(&self.peer_ids, &Internal(Accept {
                            ballot,
                            value: state.proposal.unwrap(),
                        }));
                    }
                }
            }
            Internal(Accept { ballot, value }) => {
                if state.ballot <= ballot {
                    *action = "Accepting.";
                    state.ballot = ballot;
                    state.accepted = Some((ballot, value));
                    outputs.send(src, Internal(Accepted { ballot }));
                }
            }
            Internal(Accepted { ballot }) => {
                if ballot == state.ballot {
                    *action = "Recording Accepted.";
                    state.accepts.insert(src);
                    if state.accepts.len() > (self.peer_ids.len() + 1)/2 {
                        *action = "Recording Accepted. Got quorum. Deciding.";
                        state.is_decided = true;
                        outputs.broadcast(&self.peer_ids, &Internal(Decided {
                            ballot,
                            value: state.proposal.unwrap()
                        }));
                    }
                }
            }
            Internal(Decided { ballot, value }) => {
                *action = "Recording Decided.";
                state.accepted = Some((ballot, value));
                state.is_decided = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[test]
fn can_model_paxos() {
    let system = ActorSystem {
        actors: vec![
            RegisterCfg::Server(ServerCfg { rank: 0, peer_ids: vec![1, 2] }),
            RegisterCfg::Server(ServerCfg { rank: 1, peer_ids: vec![0, 2] }),
            RegisterCfg::Server(ServerCfg { rank: 2, peer_ids: vec![0, 1] }),
            RegisterCfg::Client { server_ids: vec![0], desired_value: 'X' },
            RegisterCfg::Client { server_ids: vec![1], desired_value: 'Y' },
        ],
        init_network: Vec::new(),
        lossy_network: LossyNetwork::No,
    };
    let mut checker = system.checker(KeepPaths::Yes, |_sys, state| {
        let values = response_values(&state);
        match values.as_slice() {
            [] => true,
            [v] => *v == 'X' || *v == 'Y',
            _ => false
        }
    });
    assert_eq!(checker.check(10_000), CheckResult::Pass);
    assert_eq!(checker.visited.len(), 1529);
}

fn main() {
    let args = App::new("paxos")
        .about("single decree paxos")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("client_count")
                 .help("number of clients proposing values")
                 .default_value("2")))
        .subcommand(SubCommand::with_name("spawn")
            .about("spawn with messaging over UDP"))
        .get_matches();

    match args.subcommand() {
        ("check", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client_count"));
            println!("Benchmarking Single Decree Paxos with {} clients.", client_count);

            let mut actors = vec![
                RegisterCfg::Server(ServerCfg { rank: 0, peer_ids: vec![1, 2] }),
                RegisterCfg::Server(ServerCfg { rank: 1, peer_ids: vec![0, 2] }),
                RegisterCfg::Server(ServerCfg { rank: 2, peer_ids: vec![0, 1] }),
            ];
            for i in 0..client_count {
                actors.push(RegisterCfg::Client {
                    server_ids: vec![(i % 3) as usize],
                    desired_value: ('A' as u8 + i) as char
                });
            }

            let sys = ActorSystem { actors, init_network: Vec::new(), lossy_network: LossyNetwork::No };
            let mut checker = sys.checker(KeepPaths::Yes, |_sys, state| {
                let values = response_values(&state);
                match values.as_slice() {
                    [] => true,
                    [v] => 'A' <= *v && *v <= ('A' as u8 + client_count - 1) as char,
                    _ => false
                }
            });
            checker.check_and_report();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A set of servers that implement Single Decree Paxos.");
            println!("  You can interact with the servers using netcat. Example:");
            println!("$ nc -u 0 {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<Value, ()> { value: 'X' }).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<Value, ()>).unwrap());
            println!();

            let localhost = "127.0.0.1".parse().unwrap();
            let id0 = (localhost, port + 0);
            let id1 = (localhost, port + 1);
            let id2 = (localhost, port + 2);
            let actors = vec![
                actor::spawn(RegisterCfg::Server(ServerCfg { rank: 0, peer_ids: vec![id1, id2] }), id0),
                actor::spawn(RegisterCfg::Server(ServerCfg { rank: 1, peer_ids: vec![id0, id2] }), id1),
                actor::spawn(RegisterCfg::Server(ServerCfg { rank: 2, peer_ids: vec![id0, id1] }), id2),
            ];
            for actor in actors {
                actor.join().unwrap();
            }
        }
        _ => panic!("expected subcommand")
    }
}
