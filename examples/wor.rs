//! A simple server exposing a single register that can only be written once.
//! A cluster of servers *does not* provide consensus.

use clap::*;
use stateright::actor::register::*;
use stateright::actor::spawn::*;
use stateright::actor::system::*;
use stateright::actor::*;
use stateright::checker::*;
use stateright::explorer::*;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4};

type Value = char;

#[derive(Clone)]
struct WriteOnce;

impl Actor for WriteOnce {
    type Msg = RegisterMsg<Value, ()>;
    type State = Option<Value>;

    fn init(_i: InitIn<Self>, o: &mut Out<Self>) {
        o.set_state(None);
    }

    fn next(i: NextIn<Self>, o: &mut Out<Self>) {
        let Event::Receive(src, msg) = i.event;
        match msg {
            RegisterMsg::Put(value) if i.state.is_none() => {
                o.set_state(Some(value));
            }
            RegisterMsg::Get => {
                if let Some(value) = i.state {
                    o.send(src, RegisterMsg::Respond(*value));
                }
            }
            _ => {}
        }
    }
}

/// Create a system with one server and a variable number of clients.
fn system(servers: Vec<WriteOnce>, client_count: u8) -> System<RegisterActor<char, WriteOnce>> {
    let mut actors: Vec<_> = servers.into_iter().map(RegisterActor::Server).collect();
    let server_ids: Vec<_> = (0..actors.len()).map(Id::from).collect();
    for i in 0..client_count {
        actors.push(RegisterActor::Client {
            server_ids: server_ids.clone(),
            desired_value: ('A' as u8 + i) as char,
        });
    }
    System {
        actors,
        init_network: Vec::new(),
        lossy_network: LossyNetwork::Yes, // for some extra states
    }
}

/// Build a model that checks for validity (only values sent by clients are chosen) and
/// consistency (everyone agrees). Consistency is only true if there is a single server!
fn model(
    sys: System<RegisterActor<char, WriteOnce>>,
) -> Model<'static, System<RegisterActor<char, WriteOnce>>> {
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
        properties: vec![Property::always(
            "valid and consistent",
            move |_sys, state| {
                let values = response_values(&state);
                match values.as_slice() {
                    [] => true,
                    [v] => desired_values.contains(&v),
                    _ => false,
                }
            },
        )],
        boundary: None,
    }
}

#[cfg(test)]
#[test]
fn can_model_wor() {
    use Event::Receive;
    use RegisterMsg::*;
    use SystemAction::*;

    // Consistent if only one server.
    let mut checker = model(system(vec![WriteOnce], 2)).checker();
    assert!(checker.check(10_000).is_done());
    assert_eq!(checker.counterexample("valid and consistent"), None);

    // But the consistency requirement is violated with two servers.
    let mut checker = model(system(vec![WriteOnce, WriteOnce], 2)).checker();
    assert!(checker.check(10_000).is_done());
    assert_eq!(
        checker
            .counterexample("valid and consistent")
            .map(Path::into_actions),
        Some(vec![
            Act(Id::from(0), Receive(Id::from(2), Put('A'))),
            Act(Id::from(0), Receive(Id::from(2), Get)),
            Act(Id::from(1), Receive(Id::from(3), Put('B'))),
            Act(Id::from(1), Receive(Id::from(2), Get)),
        ])
    );
}

fn main() {
    let mut app = App::new("wor")
        .about("write-once register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("check").about("model check").arg(
                Arg::with_name("client_count")
                    .help("number of clients proposing values")
                    .default_value("5"),
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
                "Model checking a write-once register with {} clients.",
                client_count
            );
            model(system(vec![WriteOnce], client_count))
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
                "Exploring state space for write-once register with {} clients on {}.",
                client_count, address
            );
            Explorer(system(vec![WriteOnce], client_count))
                .serve(address)
                .unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a write-once register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u 0 {}", port);
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Put::<char, ()>('X')).unwrap()
            );
            println!(
                "{}",
                serde_json::to_string(&RegisterMsg::Get::<char, ()>).unwrap()
            );
            println!();

            spawn(
                RegisterActor::Server(WriteOnce),
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            )
            .join()
            .unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}
