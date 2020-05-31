//! An actor system where each server exposes a single write-once register.
//! Servers do not provide consensus.

use clap::*;
use stateright::explorer::*;
use stateright::actor::*;
use stateright::actor::register::*;
use stateright::actor::spawn::*;
use stateright::actor::system::*;
use std::net::{SocketAddrV4, Ipv4Addr};
use stateright::{Property, Model};

type Value = char;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct WriteOnceState(Option<Value>);

#[derive(Clone)]
struct WriteOnceActor;

impl Actor for WriteOnceActor {
    type Msg = RegisterMsg<Value, ()>;
    type State = WriteOnceState;

    fn init(_i: InitIn<Self>, o: &mut Out<Self>) {
        o.set_state(WriteOnceState(None));
    }

    fn next(i: NextIn<Self>, o: &mut Out<Self>) {
        let Event::Receive(src, msg) = i.event;
        match msg {
            RegisterMsg::Put(value) if i.state.0.is_none() => {
                o.set_state(WriteOnceState(Some(value)));
            }
            RegisterMsg::Get => {
                if let Some(value) = i.state.0 {
                    o.send(src, RegisterMsg::Respond(value));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct WriteOnceSystem { server_count: u8, client_count: u8 }

impl System for WriteOnceSystem {
    type Actor = RegisterActor<Value, WriteOnceActor>;

    fn actors(&self) -> Vec<Self::Actor> {
        let mut actors = Vec::new();

        for _ in 0..self.server_count {
            actors.push(RegisterActor::Server(WriteOnceActor));
        }

        let server_ids: Vec<_> = (0..self.server_count as usize).map(Id::from).collect();
        for i in 0..self.client_count {
            actors.push(RegisterActor::Client {
                server_ids: server_ids.clone(),
                desired_value: ('A' as u8 + i) as char
            });
        }

        actors
    }

    fn lossy_network(&self) -> LossyNetwork {
        LossyNetwork::Yes // for some extra states
    }

    fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
        vec![Property::<SystemModel<Self>>::always("valid and consistent", |model, state| {
            let mut sole_value = None;
            for env in &state.network {
                if let RegisterMsg::Respond(v) = env.msg {
                    // check for validity: only values sent by clients are chosen
                    if v < 'A' || (('A' as u8 + model.system.client_count) as char) < v {
                        return false;
                    }

                    // check for consistency: everyone agrees (false if more than one server)
                    if let Some(sole_value) = sole_value {
                        return sole_value == v;
                    } else {
                        sole_value = Some(v);
                    }
                }
            }
            return true;
        })]
    }
}

#[cfg(test)]
#[test]
fn can_model_wor() {
    use Event::Receive;
    use RegisterMsg::*;
    use SystemAction::Act;
    use stateright::checker::Path;

    // Consistent if only one server.
    let mut checker = WriteOnceSystem { server_count: 1, client_count: 2 }.into_model().checker();
    assert!(checker.check(10_000).is_done());
    assert_eq!(checker.counterexample("valid and consistent"), None);

    // But the consistency requirement is violated with two servers.
    let mut checker = WriteOnceSystem { server_count: 2, client_count: 2 }.into_model().checker();
    assert!(checker.check(10_000).is_done());
    assert_eq!(
        checker.counterexample("valid and consistent").map(Path::into_actions),
        Some(vec![
            Act(Id::from(0), Receive(Id::from(2), Put('A'))),
            Act(Id::from(0), Receive(Id::from(2), Get)),
            Act(Id::from(1), Receive(Id::from(3), Put('B'))),
            Act(Id::from(1), Receive(Id::from(2), Get)),
        ]));
}

fn main() {
    let mut app = App::new("wor")
        .about("write-once register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("client_count")
                 .help("number of clients proposing values")
                 .default_value("5")))
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
            println!("Model checking a write-once register with {} clients.", client_count);
            WriteOnceSystem { server_count: 1, client_count }.into_model()
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client_count"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for write-once register with {} clients on {}.",
                client_count, address);
            Explorer(WriteOnceSystem { server_count: 1, client_count }.into_model()).serve(address).unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a write-once register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u 0 {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<char, ()>('X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<char, ()>).unwrap());
            println!();

            spawn(RegisterActor::Server(WriteOnceActor), SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).join().unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}

