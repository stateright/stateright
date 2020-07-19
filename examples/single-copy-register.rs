//! An actor system where each server exposes a rewritable single-copy register. Servers do not
//! provide consensus.

use clap::*;
use stateright::actor::{Actor, Id, Out};
use stateright::actor::register::{RegisterMsg, TestRequestId, TestValue};
use stateright::actor::register::RegisterMsg::*;
use stateright::actor::register::RegisterTestSystem;
use stateright::actor::spawn::spawn;
use stateright::actor::system::{System, LossyNetwork};
use stateright::explorer::Explorer;
use stateright::Model;
use std::net::{SocketAddrV4, Ipv4Addr};

#[derive(Clone)]
struct SingleCopyActor;

impl Actor for SingleCopyActor {
    type Msg = RegisterMsg<TestRequestId, TestValue, ()>;
    type State = Option<TestValue>;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) {
        o.set_state(None);
    }

    fn on_msg(&self, _id: Id, state: &Self::State, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Put(req_id, value) => {
                o.set_state(Some(value));
                o.send(src, PutOk(req_id));
            }
            Get(req_id) => {
                o.send(src, GetOk(req_id, state.clone()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[test]
fn can_model_single_copy_register() {
    use stateright::Model;
    use stateright::actor::system::SystemAction::Deliver;

    // Consistent if only one server.
    let mut checker = RegisterTestSystem {
        servers: vec![SingleCopyActor],
        put_count: 3,
        get_count: 2,
        .. Default::default()
    }.into_model().checker();
    checker.check(10_000).assert_properties();
    assert_eq!(
        checker.assert_example("value chosen").into_actions(),
        vec![
            Deliver { src: Id::from(998), dst: Id::from(0), msg: Put(1, 'B') },
            Deliver { src: Id::from(995), dst: Id::from(0), msg: Get(4) },
        ]);
    assert_eq!(checker.generated_count(), 2_104);

    // But the consistency requirement is violated with two servers.
    let mut checker = RegisterTestSystem {
        servers: vec![SingleCopyActor, SingleCopyActor],
        put_count: 3,
        get_count: 2,
        .. Default::default()
    }.into_model().checker();
    checker.check(10_000);
    assert_eq!(
        checker.assert_counterexample("linearizable").into_actions(),
        vec![
            Deliver { src: Id::from(998), dst: Id::from(1), msg: Put(1, 'B') },
            Deliver { src: Id::from(996), dst: Id::from(0), msg: Get(3) },
        ]);
    assert_eq!(
        checker.assert_example("value chosen").into_actions(),
        vec![
            Deliver { src: Id::from(998), dst: Id::from(1), msg: Put(1, 'B') },
            Deliver { src: Id::from(995), dst: Id::from(1), msg: Get(4) },
        ]);
    assert_eq!(checker.generated_count(), 500); // fewer states b/c invariant violation found
}

fn main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("debug"));

    let mut app = App::new("wor")
        .about("single-copy register")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("put_count")
                .help("number of puts")
                .default_value("2"))
            .arg(Arg::with_name("get_count")
                .help("number of gets")
                .default_value("2")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("put_count")
                .help("number of puts")
                .default_value("2"))
            .arg(Arg::with_name("get_count")
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
            let put_count = std::cmp::min(
                26, value_t!(args, "put_count", u8).expect("put count missing"));
            let get_count = std::cmp::min(
                26, value_t!(args, "get_count", u8).expect("get count missing"));
            println!("Model checking a single-copy register with {} puts and {} gets.",
                     put_count, get_count);
            RegisterTestSystem {
                servers: vec![SingleCopyActor],
                put_count,
                get_count,
                lossy_network: LossyNetwork::Yes, // for extra states
                .. Default::default()
            }.into_model()
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let put_count = std::cmp::min(
                26, value_t!(args, "put_count", u8).expect("put count missing"));
            let get_count = std::cmp::min(
                26, value_t!(args, "get_count", u8).expect("get count missing"));
            let address = value_t!(args, "address", String).expect("address");
            println!(
                "Exploring state space for single-copy register with {} puts and {} gets on {}.",
                put_count, get_count, address);
            RegisterTestSystem {
                servers: vec![SingleCopyActor],
                put_count,
                get_count,
                lossy_network: LossyNetwork::Yes, // for extra states
                .. Default::default()
            }.into_model().checker().serve(address).unwrap();
        }
        ("spawn", Some(_args)) => {
            let port = 3000;

            println!("  A server that implements a single-copy register.");
            println!("  You can interact with the server using netcat. Example:");
            println!("$ nc -u localhost {}", port);
            println!("{}", serde_json::to_string(&RegisterMsg::Put::<TestRequestId, TestValue, ()>(1, 'X')).unwrap());
            println!("{}", serde_json::to_string(&RegisterMsg::Get::<TestRequestId, TestValue, ()>(2)).unwrap());
            println!();

            spawn(SingleCopyActor, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).join().unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}