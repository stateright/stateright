//! A cluster that implements Single Decree Paxos.

use clap::*;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use stateright::actor::register::RegisterMsg::*;
use stateright::actor::register::*;
use stateright::actor::spawn::*;
use stateright::actor::system::*;
use stateright::actor::*;
use stateright::checker::*;
use stateright::explorer::*;
use std::collections::*;
use std::net::{Ipv4Addr, SocketAddrV4};

type Round = u32;
type Rank = u32;
type Ballot = (Round, Rank);
type Value = char;

#[derive(Clone)]
struct Paxos {
    rank: Rank,
    peer_ids: Vec<Id>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
enum PaxosMsg {
    Prepare {
        ballot: Ballot,
    },
    Prepared {
        ballot: Ballot,
        last_accepted: Option<(Ballot, Value)>,
    },

    Accept {
        ballot: Ballot,
        value: Value,
    },
    Accepted {
        ballot: Ballot,
    },

    Decided {
        ballot: Ballot,
        value: Value,
    },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct PaxosState {
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

impl Actor for Paxos {
    type Msg = RegisterMsg<Value, PaxosMsg>;
    type State = PaxosState;

    fn init(_i: InitIn<Self>, o: &mut Out<Self>) {
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

    fn next(i: NextIn<Self>, o: &mut Out<Self>) {
        use crate::PaxosMsg::*;
        let Event::Receive(src, msg) = i.event;
        match msg {
            Put(value) if !i.state.is_decided && i.state.proposal.is_none() => {
                let mut state = i.state.clone();
                state.ballot = (state.ballot.0 + 1, i.context.rank);
                state.proposal = Some(value);
                state.prepares = Default::default();
                state.accepts = Default::default();
                o.broadcast(
                    &i.context.peer_ids,
                    &Internal(Prepare {
                        ballot: state.ballot,
                    }),
                );
                o.set_state(state);
            }
            Get if i.state.is_decided => {
                if let Some((_ballot, value)) = i.state.accepted {
                    o.send(src, Respond(value));
                }
            }
            Internal(Prepare { ballot }) if i.state.ballot < ballot => {
                let mut state = i.state.clone();
                state.ballot = ballot;
                o.set_state(state);
                o.send(
                    src,
                    Internal(Prepared {
                        ballot,
                        last_accepted: i.state.accepted,
                    }),
                );
            }
            Internal(Prepared {
                ballot,
                last_accepted,
            }) if ballot == i.state.ballot => {
                let mut state = i.state.clone();
                state.prepares.insert(src, last_accepted);
                if state.prepares.len() > (i.context.peer_ids.len() + 1) / 2 {
                    state.proposal = state
                        .prepares
                        .values()
                        .max()
                        .unwrap()
                        .map(|(_b, v)| v)
                        .or(state.proposal);
                    state.accepted = Some((ballot, state.proposal.unwrap()));
                    o.broadcast(
                        &i.context.peer_ids,
                        &Internal(Accept {
                            ballot,
                            value: state.proposal.unwrap(),
                        }),
                    );
                }
                o.set_state(state);
            }
            Internal(Accept { ballot, value }) if i.state.ballot <= ballot => {
                let mut state = i.state.clone();
                state.ballot = ballot;
                state.accepted = Some((ballot, value));
                o.set_state(state);
                o.send(src, Internal(Accepted { ballot }));
            }
            Internal(Accepted { ballot }) if ballot == i.state.ballot => {
                let mut state = i.state.clone();
                state.accepts.insert(src);
                if state.accepts.len() > (i.context.peer_ids.len() + 1) / 2 {
                    state.is_decided = true;
                    o.broadcast(
                        &i.context.peer_ids,
                        &Internal(Decided {
                            ballot,
                            value: state.proposal.unwrap(),
                        }),
                    );
                }
                o.set_state(state);
            }
            Internal(Decided { ballot, value }) => {
                let mut state = i.state.clone();
                state.accepted = Some((ballot, value));
                state.is_decided = true;
                o.set_state(state);
            }
            _ => {}
        }
    }
}

/// Create a system with 3 servers and a variable number of clients.
fn system(client_count: u8) -> System<RegisterActor<char, Paxos>> {
    let mut actors = vec![
        RegisterActor::Server(Paxos {
            rank: 0,
            peer_ids: vec![Id::from(1), Id::from(2)],
        }),
        RegisterActor::Server(Paxos {
            rank: 1,
            peer_ids: vec![Id::from(0), Id::from(2)],
        }),
        RegisterActor::Server(Paxos {
            rank: 2,
            peer_ids: vec![Id::from(0), Id::from(1)],
        }),
    ];
    for i in 0..client_count {
        actors.push(RegisterActor::Client {
            server_ids: vec![Id::from((i % 3) as usize)], // one for each client
            desired_value: ('A' as u8 + i) as char,
        });
    }
    System {
        actors,
        init_network: Vec::with_capacity(20),
        lossy_network: LossyNetwork::No,
    }
}

/// Build a model that checks for validity (only values sent by clients are chosen) and
/// consistency (everyone agrees).
fn model(
    sys: System<RegisterActor<char, Paxos>>,
) -> Model<'static, System<RegisterActor<char, Paxos>>> {
    let desired_values: BTreeSet<_> = sys
        .actors
        .iter()
        .filter_map(|actor| {
            if let RegisterActor::Client { desired_value, .. } = actor {
                Some(desired_value.clone())
            } else {
                None
            }
        })
        .collect();
    Model {
        state_machine: sys,
        properties: vec![
            Property::sometimes("value chosen", move |_sys, state| {
                !response_values(&state).is_empty()
            }),
            Property::always("valid and consistent", move |_sys, state| {
                let values = response_values(&state);
                match values.as_slice() {
                    [] => true,
                    [v] => desired_values.contains(&v),
                    _ => false,
                }
            }),
        ],
        boundary: Some(Box::new(|_sys, state| {
            state.actor_states.iter().all(|s| {
                if let RegisterActorState::Server(ref state) = **s {
                    state.ballot.0 < 4
                } else {
                    true
                }
            })
        })),
    }
}

#[cfg(test)]
#[test]
fn can_model_paxos() {
    use Event::*;
    use PaxosMsg::*;
    use SystemAction::*;
    let mut checker = model(system(2)).checker();
    assert_eq!(checker.check(10_000).is_done(), true);
    assert_eq!(checker.generated_count(), 1529);
    assert_eq!(
        checker.example("value chosen").map(Path::into_actions),
        Some(vec![
            Act(Id::from(0), Receive(Id::from(3), Put('A'))),
            Act(
                Id::from(1),
                Receive(Id::from(0), Internal(Prepare { ballot: (1, 0) }))
            ),
            Act(
                Id::from(2),
                Receive(Id::from(0), Internal(Prepare { ballot: (1, 0) }))
            ),
            Act(
                Id::from(0),
                Receive(
                    Id::from(1),
                    Internal(Prepared {
                        ballot: (1, 0),
                        last_accepted: None
                    })
                )
            ),
            Act(
                Id::from(0),
                Receive(
                    Id::from(2),
                    Internal(Prepared {
                        ballot: (1, 0),
                        last_accepted: None
                    })
                )
            ),
            Act(
                Id::from(1),
                Receive(
                    Id::from(0),
                    Internal(Accept {
                        ballot: (1, 0),
                        value: 'A'
                    })
                )
            ),
            Act(
                Id::from(2),
                Receive(
                    Id::from(0),
                    Internal(Accept {
                        ballot: (1, 0),
                        value: 'A'
                    })
                )
            ),
            Act(
                Id::from(0),
                Receive(Id::from(1), Internal(Accepted { ballot: (1, 0) }))
            ),
            Act(
                Id::from(0),
                Receive(Id::from(2), Internal(Accepted { ballot: (1, 0) }))
            ),
            Act(Id::from(0), Receive(Id::from(3), Get)),
        ])
    );
    assert_eq!(checker.counterexample("valid and consistent"), None);
}

fn main() {
    let mut app = clap::App::new("paxos")
        .about("single decree paxos")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("check").about("model check").arg(
                Arg::with_name("client_count")
                    .help("number of clients proposing values")
                    .default_value("2"),
            ),
        )
        .subcommand(
            SubCommand::with_name("explore")
                .about("interactively explore state space")
                .arg(
                    Arg::with_name("client_count")
                        .help("number of clients proposing values")
                        .default_value("2"),
                )
                .arg(
                    Arg::with_name("address")
                        .help("address Explorer service should listen upon")
                        .default_value("localhost:3000"),
                ),
        )
        .subcommand(SubCommand::with_name("spawn").about("spawn with messaging over UDP"));
    let args = app.clone().get_matches();

    match args.subcommand() {
        ("check", Some(args)) => {
            let client_count = std::cmp::min(
                26,
                value_t!(args, "client_count", u8).expect("client_count"),
            );
            println!(
                "Model checking Single Decree Paxos with {} clients.",
                client_count
            );
            model(system(client_count))
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26,
                value_t!(args, "client_count", u8).expect("client_count"),
            );
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for Single Decree Paxos with {} clients on {}.",
                client_count, address
            );
            Explorer(system(client_count)).serve(address).unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A set of servers that implement Single Decree Paxos.");
            println!("  You can monitor and interact using tcpdump and netcat. Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u 0 {}", port);
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Put::<Value, ()>('X')).unwrap()
            );
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Get::<Value, ()>).unwrap()
            );
            println!();

            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 0));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            let actors = vec![
                spawn(
                    RegisterActor::Server(Paxos {
                        rank: 0,
                        peer_ids: vec![id1, id2],
                    }),
                    id0,
                ),
                spawn(
                    RegisterActor::Server(Paxos {
                        rank: 1,
                        peer_ids: vec![id0, id2],
                    }),
                    id1,
                ),
                spawn(
                    RegisterActor::Server(Paxos {
                        rank: 2,
                        peer_ids: vec![id0, id1],
                    }),
                    id2,
                ),
            ];
            for actor in actors {
                actor.join().unwrap();
            }
        }
        _ => app.print_help().unwrap(),
    }
}
