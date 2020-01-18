//! A cluster that implements Single Decree Paxos.

use clap::*;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use stateright::actor::*;
use stateright::actor::register::*;
use stateright::actor::register::RegisterMsg::*;
use stateright::actor::spawn::*;
use stateright::actor::system::*;
use stateright::checker::*;
use stateright::explorer::*;
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

#[derive(Clone)]
struct ServerCfg<Id> { rank: Rank, peer_ids: Vec<Id> }

impl<Id: Copy + Ord> Actor<Id> for ServerCfg<Id> {
    type Msg = RegisterMsg<Value, ServerMsg>;
    type State = ServerState<Id>;

    fn start(&self) -> ActorResult<Id, Self::Msg, Self::State> {
        ActorResult::start(ServerState {
            // shared state
            ballot: (0, 0),

            // leader state
            proposal: None,
            prepares: BTreeMap::new(),
            accepts: BTreeSet::new(),

            // acceptor state
            accepted: None,
            is_decided: false,
        }, |_outputs| {})
    }

    fn advance(&self, state: &Self::State, input: &ActorInput<Id, Self::Msg>)
            -> Option<ActorResult<Id, Self::Msg, Self::State>> {
        use crate::ServerMsg::*;

        let ActorInput::Deliver { src, msg } = input.clone(); // clone makes following code clearer
        match msg {
            Put { value } if !state.is_decided => {
                // reduce state space until upcoming model checking optimizations land
                if state.proposal.is_some() || state.ballot.0 == 3 { return None; }

                return ActorResult::advance(state, |state, outputs| {
                    state.ballot = (state.ballot.0 + 1, self.rank);
                    state.proposal = Some(value);
                    state.prepares = Default::default();
                    state.accepts = Default::default();
                    outputs.broadcast(
                        &self.peer_ids,
                        &Internal(Prepare { ballot: state.ballot }));
                });
            }
            Get if state.is_decided => {
                if let Some((_ballot, value)) = state.accepted {
                    return ActorResult::advance(state, |_state, outputs| {
                        outputs.send(src, Respond { value });
                    });
                }
            }
            Internal(Prepare { ballot }) if state.ballot < ballot => {
                return ActorResult::advance(state, |state, outputs| {
                    state.ballot = ballot;
                    outputs.send(src, Internal(Prepared {
                        ballot,
                        last_accepted: state.accepted,
                    }));
                });
            }
            Internal(Prepared { ballot, last_accepted }) if ballot == state.ballot => {
                return ActorResult::advance(state, |state, outputs| {
                    state.prepares.insert(src, last_accepted);
                    if state.prepares.len() > (self.peer_ids.len() + 1)/2 {
                        state.proposal = state.prepares
                            .values().max().unwrap().map(|(_b,v)| v)
                            .or(state.proposal);
                        state.accepted = Some((ballot, state.proposal.unwrap()));
                        outputs.broadcast(&self.peer_ids, &Internal(Accept {
                            ballot,
                            value: state.proposal.unwrap(),
                        }));
                    }
                });
            }
            Internal(Accept { ballot, value }) if state.ballot <= ballot => {
                return ActorResult::advance(state, |state, outputs| {
                    state.ballot = ballot;
                    state.accepted = Some((ballot, value));
                    outputs.send(src, Internal(Accepted { ballot }));
                });
            }
            Internal(Accepted { ballot }) if ballot == state.ballot => {
                return ActorResult::advance(state, |state, outputs| {
                    state.accepts.insert(src);
                    if state.accepts.len() > (self.peer_ids.len() + 1)/2 {
                        state.is_decided = true;
                        outputs.broadcast(&self.peer_ids, &Internal(Decided {
                            ballot,
                            value: state.proposal.unwrap()
                        }));
                    }
                });
            }
            Internal(Decided { ballot, value }) => {
                return ActorResult::advance(state, |state, _outputs| {
                    state.accepted = Some((ballot, value));
                    state.is_decided = true;
                });
            }
            _ => {}
        }
        return None;
    }
}

/// Create a system with 3 servers and a variable number of clients.
fn system(client_count: u8)
        -> ActorSystem<RegisterCfg<ModelId, char, ServerCfg<ModelId>>> {
    let mut actors = vec![
        RegisterCfg::Server(ServerCfg { rank: 0, peer_ids: vec![1, 2] }),
        RegisterCfg::Server(ServerCfg { rank: 1, peer_ids: vec![0, 2] }),
        RegisterCfg::Server(ServerCfg { rank: 2, peer_ids: vec![0, 1] }),
    ];
    for i in 0..client_count {
        actors.push(RegisterCfg::Client {
            server_ids: vec![(i % 3) as usize], // one for each client
            desired_value: ('A' as u8 + i) as char
        });
    }
    ActorSystem {
        actors,
        init_network: Vec::with_capacity(20),
        lossy_network: LossyNetwork::No,
    }
}

/// Build a model that checks for validity (only values sent by clients are chosen) and
/// consistency (everyone agrees).
fn model(sys: ActorSystem<RegisterCfg<ModelId, char, ServerCfg<ModelId>>>)
        -> Model<'static, ActorSystem<RegisterCfg<ModelId, char, ServerCfg<ModelId>>>> {
    let desired_values: BTreeSet<_> = sys.actors.iter()
        .filter_map(|actor| {
            if let RegisterCfg::Client { desired_value, .. } = actor {
                Some(desired_value.clone())
            } else {
                None
            }
        })
        .collect();
    Model {
        state_machine: sys,
        properties: vec![
            Property::sometimes("value chosen", move |_sys, snap| {
                !response_values(&snap).is_empty()
            }),
            Property::always("valid and consistent", move |_sys, snap| {
                let values = response_values(&snap);
                match values.as_slice() {
                    [] => true,
                    [v] => desired_values.contains(&v),
                    _ => false
                }
            }),
        ],
    }
}

#[cfg(test)]
#[test]
fn can_model_paxos() {
    use ActorSystemAction::*;
    use ServerMsg::*;
    use ActorInput::*;
    let mut checker = model(system(2)).checker();
    assert_eq!(checker.check(10_000).is_done(), true);
    assert_eq!(checker.generated_count(), 1529);
    assert_eq!(checker.example("value chosen").map(Path::into_actions), Some(vec![
        Act(0, Deliver { src: 3, msg: Put { value: 'A' } }),
        Act(1, Deliver { src: 0, msg: Internal(Prepare { ballot: (1, 0) }) }),
        Act(2, Deliver { src: 0, msg: Internal(Prepare { ballot: (1, 0) }) }),
        Act(0, Deliver { src: 1, msg: Internal(Prepared { ballot: (1, 0), last_accepted: None }) }),
        Act(0, Deliver { src: 2, msg: Internal(Prepared { ballot: (1, 0), last_accepted: None }) }), 
        Act(1, Deliver { src: 0, msg: Internal(Accept { ballot: (1, 0), value: 'A' }) }),
        Act(2, Deliver { src: 0, msg: Internal(Accept { ballot: (1, 0), value: 'A' }) }), 
        Act(0, Deliver { src: 1, msg: Internal(Accepted { ballot: (1, 0) }) }),
        Act(0, Deliver { src: 2, msg: Internal(Accepted { ballot: (1, 0) }) }), 
        Act(0, Deliver { src: 3, msg: Get }),
    ]));
    assert_eq!(checker.counterexample("valid and consistent"), None);
}

fn main() {
    let mut app = clap::App::new("paxos")
        .about("single decree paxos")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("client_count")
                 .help("number of clients proposing values")
                 .default_value("2")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("client_count")
                .help("number of clients proposing values")
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
                26, value_t!(args, "client_count", u8).expect("client_count"));
            println!("Model checking Single Decree Paxos with {} clients.", client_count);
            model(system(client_count))
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client_count"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for Single Decree Paxos with {} clients on {}.",
                client_count, address);
            Explorer(system(client_count)).serve(address).unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A set of servers that implement Single Decree Paxos.");
            println!("  You can monitor and interact using tcpdump and netcat. Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u 0 {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<Value, ()> { value: 'X' }).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<Value, ()>).unwrap());
            println!();

            let localhost = "127.0.0.1".parse().unwrap();
            let id0 = (localhost, port + 0);
            let id1 = (localhost, port + 1);
            let id2 = (localhost, port + 2);
            let actors = vec![
                spawn(RegisterCfg::Server(ServerCfg { rank: 0, peer_ids: vec![id1, id2] }), id0),
                spawn(RegisterCfg::Server(ServerCfg { rank: 1, peer_ids: vec![id0, id2] }), id1),
                spawn(RegisterCfg::Server(ServerCfg { rank: 2, peer_ids: vec![id0, id1] }), id2),
            ];
            for actor in actors {
                actor.join().unwrap();
            }
        }
        _ => app.print_help().unwrap(),
    }
}
