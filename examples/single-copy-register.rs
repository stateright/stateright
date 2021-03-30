//! An actor system where each server exposes a rewritable single-copy register. Servers do not
//! provide consensus.

use stateright::{Checker, Model};
use stateright::actor::{Actor, DuplicatingNetwork, Id, Out};
use stateright::actor::register::{RegisterMsg, RegisterMsg::*, RegisterCfg, TestRequestId, TestValue};
use std::borrow::Cow;

#[derive(Clone)]
struct SingleCopyActor;

impl Actor for SingleCopyActor {
    type Msg = RegisterMsg<TestRequestId, TestValue, ()>;
    type State = TestValue;

    fn on_start(&self, _id: Id, _o: &mut Out<Self>) -> Self::State {
        TestValue::default()
    }

    fn on_msg(&self, _id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Put(req_id, value) => {
                *state.to_mut() = value;
                o.send(src, PutOk(req_id));
            }
            Get(req_id) => {
                o.send(src, GetOk(req_id, **state));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[test]
fn can_model_single_copy_register() {
    use stateright::actor::DuplicatingNetwork;
    use stateright::actor::ActorModelAction::Deliver;

    // Linearizable if only one server. DFS for this one.
    let checker = RegisterCfg {
            servers: vec![SingleCopyActor],
            client_count: 2,
        }
        .into_model()
        .duplicating_network(DuplicatingNetwork::No)
        .checker().spawn_dfs().join();
    checker.assert_properties();
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(2), dst: Id::from(0), msg: Put(2, 'B') },
        Deliver { src: Id::from(0), dst: Id::from(2), msg: PutOk(2) },
        Deliver { src: Id::from(2), dst: Id::from(0), msg: Get(4) },
    ]);
    assert_eq!(checker.generated_count(), 180);

    // Otherwise (if more than one server) then not linearizabile. BFS this time.
    let checker = RegisterCfg {
            servers: vec![SingleCopyActor, SingleCopyActor],
            client_count: 2,
        }
        .into_model()
        .duplicating_network(DuplicatingNetwork::No)
        .checker().spawn_bfs().join();
    checker.assert_discovery("linearizable", vec![
        Deliver { src: Id::from(3), dst: Id::from(1), msg: Put(3, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(3), msg: PutOk(3) },
        Deliver { src: Id::from(3), dst: Id::from(0), msg: Get(6) },
        Deliver { src: Id::from(0), dst: Id::from(3), msg: GetOk(6, '\u{0}') },
    ]);
    checker.assert_discovery("value chosen", vec![
        Deliver { src: Id::from(3), dst: Id::from(1), msg: Put(3, 'B') },
        Deliver { src: Id::from(1), dst: Id::from(3), msg: PutOk(3) },
        Deliver { src: Id::from(2), dst: Id::from(0), msg: Put(2, 'A') },
        Deliver { src: Id::from(3), dst: Id::from(0), msg: Get(6) },
    ]);
    assert_eq!(checker.generated_count(), 20);
}

fn main() {
    use clap::{App, AppSettings, Arg, SubCommand, value_t};
    use stateright::actor::spawn;
    use std::net::{SocketAddrV4, Ipv4Addr};

    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut app = App::new("wor")
        .about("single-copy register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("client_count")
                .help("number of gets")
                .default_value("2")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("client_count")
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
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client count missing"));
            println!("Model checking a single-copy register with {} clients.",
                     client_count);
            RegisterCfg {
                    servers: vec![SingleCopyActor],
                    client_count,
                }
                .into_model()
                .duplicating_network(DuplicatingNetwork::No)
                .checker()
                .threads(num_cpus::get()).spawn_dfs()
                .report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let client_count = std::cmp::min(
                26, value_t!(args, "client_count", u8).expect("client count missing"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for single-copy register with {} clients on {}.",
                client_count, address);
            RegisterCfg {
                    servers: vec![SingleCopyActor],
                    client_count,
                }
                .into_model()
                .duplicating_network(DuplicatingNetwork::No)
                .checker()
                .threads(num_cpus::get())
                .serve(address);
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a single-copy register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u localhost {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<TestRequestId, TestValue, ()>(1, 'X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<TestRequestId, TestValue, ()>(2)).unwrap());
            println!();

            // WARNING: Omits `ordered_reliable_link` to keep the message
            //          protocol simple for `nc`.
            spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), SingleCopyActor)
                ]).unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}
