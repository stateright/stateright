//! A cluster that implements Single Decree Paxos.

use clap::*;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use stateright::actor::*;
use stateright::actor::register::*;
use stateright::actor::register::RegisterMsg::*;
use stateright::actor::spawn::*;
use stateright::actor::system::*;
use stateright::explorer::*;
use stateright::util::{HashableHashMap, HashableHashSet};
use std::net::{SocketAddrV4, Ipv4Addr};
use stateright::{Property, Model};

type Round = u32;
type Rank = u32;
type Ballot = (Round, Rank);
type Value = char;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
enum PaxosMsg {
    Prepare { ballot: Ballot },
    Prepared { ballot: Ballot, last_accepted: Option<(Ballot, Value)> },

    Accept { ballot: Ballot, value: Value },
    Accepted { ballot: Ballot },

    Decided { ballot: Ballot, value: Value },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PaxosState {
    // shared state
    ballot: Ballot,

    // leader state
    proposal: Option<Value>,
    prepares: HashableHashMap<Id, Option<(Ballot, Value)>>,
    accepts: HashableHashSet<Id>,

    // acceptor state
    accepted: Option<(Ballot, Value)>,
    is_decided: bool,
}

#[derive(Clone)]
struct PaxosActor { rank: Rank, peer_ids: Vec<Id> }

impl Actor for PaxosActor {
    type Msg = RegisterMsg<Value, PaxosMsg>;
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
        use crate::PaxosMsg::*;
        match msg {
            Put(value) if !state.is_decided && state.proposal.is_none() => {
                let mut state = state.clone();
                state.ballot = (state.ballot.0 + 1, self.rank);
                state.proposal = Some(value);
                state.prepares = Default::default();
                state.accepts = Default::default();
                o.broadcast(
                    &self.peer_ids,
                    &Internal(Prepare { ballot: state.ballot }));
                o.set_state(state);
            }
            Get if state.is_decided => {
                if let Some((_ballot, value)) = state.accepted {
                    o.send(src, Respond(value));
                }
            }
            Internal(Prepare { ballot }) if state.ballot < ballot => {
                let mut state = state.clone();
                state.ballot = ballot;
                o.send(src, Internal(Prepared {
                    ballot,
                    last_accepted: state.accepted,
                }));
                o.set_state(state);
            }
            Internal(Prepared { ballot, last_accepted }) if ballot == state.ballot => {
                let mut state = state.clone();
                state.prepares.insert(src, last_accepted);
                if state.prepares.len() > (self.peer_ids.len() + 1) / 2 {
                    state.proposal = state.prepares
                        .values().max().unwrap().map(|(_b, v)| v)
                        .or(state.proposal);
                    state.accepted = Some((ballot, state.proposal.unwrap()));
                    o.broadcast(&self.peer_ids, &Internal(Accept {
                        ballot,
                        value: state.proposal.unwrap(),
                    }));
                }
                o.set_state(state);
            }
            Internal(Accept { ballot, value }) if state.ballot <= ballot => {
                let mut state = state.clone();
                state.ballot = ballot;
                state.accepted = Some((ballot, value));
                o.set_state(state);
                o.send(src, Internal(Accepted { ballot }));
            }
            Internal(Accepted { ballot }) if ballot == state.ballot => {
                let mut state = state.clone();
                state.accepts.insert(src);
                if state.accepts.len() > (self.peer_ids.len() + 1) / 2 {
                    state.is_decided = true;
                    o.broadcast(&self.peer_ids, &Internal(Decided {
                        ballot,
                        value: state.proposal.unwrap()
                    }));
                }
                o.set_state(state);
            }
            Internal(Decided { ballot, value }) => {
                let mut state = state.clone();
                state.accepted = Some((ballot, value));
                state.is_decided = true;
                o.set_state(state);
            }
            _ => {}
        }
    }
}

/// A Paxos actor system with 3 servers and a variable number of clients.
#[derive(Clone)]
struct PaxosSystem { client_count: u8 }

impl System for PaxosSystem {
    type Actor = RegisterActor<Value, PaxosActor>;
    type History = ();

    fn actors(&self) -> Vec<Self::Actor> {
        let mut actors = vec![
            RegisterActor::Server(PaxosActor { rank: 0, peer_ids: vec![Id::from(1), Id::from(2)] }),
            RegisterActor::Server(PaxosActor { rank: 1, peer_ids: vec![Id::from(0), Id::from(2)] }),
            RegisterActor::Server(PaxosActor { rank: 2, peer_ids: vec![Id::from(0), Id::from(1)] }),
        ];
        for i in 0..self.client_count {
            actors.push(RegisterActor::Client {
                server_ids: vec![Id::from((i % 3) as usize)], // one for each client
                desired_value: ('A' as u8 + i) as char
            });
        }
        actors
    }

    fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
        vec![
            Property::<SystemModel<Self>>::sometimes("value chosen",  |_, state| {
                for env in &state.network {
                    if let RegisterMsg::Respond(_v) = env.msg {
                        return true;
                    }
                }
                return false
            }),
            Property::<SystemModel<Self>>::always("valid and consistent", |model, state| {
                let mut sole_value = None;
                for env in &state.network {
                    if let RegisterMsg::Respond(v) = env.msg {
                        // check for validity: only values sent by clients are chosen
                        if v < 'A' || (('A' as u8 + model.system.client_count) as char) < v {
                            return false;
                        }

                        // check for consistency: everyone agrees
                        if let Some(sole_value) = sole_value {
                            if v != sole_value {
                                return false;
                            }
                        } else {
                            sole_value = Some(v);
                        }
                    }
                }
                return true;
            }),
        ]
    }

    fn within_boundary(&self, state: &SystemState<Self>) -> bool {
        state.actor_states.iter().all(|s| {
            if let RegisterActorState::Server(ref state) = **s {
                state.ballot.0 < 4
            } else {
                true
            }
        })
    }
}

#[cfg(test)]
#[test]
fn can_model_paxos() {
    use PaxosMsg::*;
    use SystemAction::Deliver;

    let mut checker = PaxosSystem { client_count: 2 }.into_model().checker();
    checker.check(10_000).assert_properties();
    assert_eq!(checker.generated_count(), 1529);
    assert_eq!(checker.assert_example("value chosen").into_actions(), vec![
        Deliver { src: Id::from(4), dst: Id::from(1), msg: Put('B') },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Prepare { ballot: (1, 1) }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Prepared { ballot: (1, 1), last_accepted: None }) },
        Deliver { src: Id::from(1), dst: Id::from(0), msg: Internal(Accept { ballot: (1, 1), value: 'B' }) },
        Deliver { src: Id::from(1), dst: Id::from(2), msg: Internal(Accept { ballot: (1, 1), value: 'B' }) },
        Deliver { src: Id::from(2), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(0), dst: Id::from(1), msg: Internal(Accepted { ballot: (1, 1) }) },
        Deliver { src: Id::from(4), dst: Id::from(1), msg: Get },
    ]);
}

fn main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("debug"));

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
            PaxosSystem { client_count }.into_model()
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
            PaxosSystem { client_count }
                .into_model().checker().serve(address).unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A set of servers that implement Single Decree Paxos.");
            println!("  You can monitor and interact using tcpdump and netcat. Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u 0 {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<Value, ()>('X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<Value, ()>).unwrap());
            println!();

            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 0));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            let actors = vec![
                spawn(RegisterActor::Server(PaxosActor { rank: 0, peer_ids: vec![id1, id2] }), id0),
                spawn(RegisterActor::Server(PaxosActor { rank: 1, peer_ids: vec![id0, id2] }), id1),
                spawn(RegisterActor::Server(PaxosActor { rank: 2, peer_ids: vec![id0, id1] }), id2),
            ];
            for actor in actors {
                actor.join().unwrap();
            }
        }
        _ => app.print_help().unwrap(),
    }
}
