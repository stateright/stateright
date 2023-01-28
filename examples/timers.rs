use serde::{Deserialize, Serialize};
use stateright::report::WriteReporter;
use stateright::actor::model_timeout;
use stateright::{Model, Checker};
use stateright::actor::{
    Actor, ActorModel, Id, model_peers, Network,Out};
use std::borrow::Cow;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
enum PingerMsg {
    Ping,
    Pong,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Serialize, Deserialize)]
enum PingerTimer {
    Even,
    Odd,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PingerState {
    received: usize
}

#[derive(Clone)]
struct PingerActor { peer_ids: Vec<Id> }

impl Actor for PingerActor {
    type Msg = PingerMsg;
    type State = PingerState;
    type Timer = PingerTimer;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) -> Self::State {
        o.set_timer(PingerTimer::Even, model_timeout());
        o.set_timer(PingerTimer::Odd, model_timeout());
        PingerState { received: 0 }
    }

    fn on_msg(&self, _id: Id, state: &mut Cow<Self::State>, src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            PingerMsg::Ping => {
                state.to_mut().received += 2;
                o.send(src, PingerMsg::Pong);
            }
            PingerMsg::Pong => {
                state.to_mut().received += 1;
            }
        }
    }

    fn on_timeout(&self, _id: Id, _state: &mut Cow<Self::State>, timer: &Self::Timer, o: &mut Out<Self>) {
        match timer {
            PingerTimer::Even => {
                o.set_timer(PingerTimer::Even, model_timeout());
                for &dst in &self.peer_ids {
                    if usize::from(dst) % 2 == 0 {
                        o.send(dst, PingerMsg::Ping);
                    }
                }
            }
            PingerTimer::Odd => {
                o.set_timer(PingerTimer::Odd, model_timeout());
                for &dst in &self.peer_ids {
                    if usize::from(dst) % 2 != 0 {
                        o.send(dst, PingerMsg::Ping);
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct PingerModelCfg {
    server_count: usize,
    network: Network<<PingerActor as Actor>::Msg>,
}

impl PingerModelCfg {
    fn into_model(self) ->
        ActorModel<
            PingerActor,
            Self,
            ()>
    {
        ActorModel::new(
                self.clone(),
                (),
            )
            .actors((0..self.server_count)
                .map(|i| PingerActor {
                    peer_ids: model_peers(i, self.server_count),
                }))
            .init_network(self.network)
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let network = args.opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]))
                .into();
            println!("Model checking Pingers");
            PingerModelCfg {
                    server_count: 3,
                    network,
                }
                .into_model().checker().threads(num_cpus::get())
                .spawn_dfs().report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let address = args.opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            let network = args.opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]))
                .into();
            println!(
                "Exploring state space for Pingers on {}.",
                address);
            PingerModelCfg {
                    server_count: 3,
                    network,
                }
                .into_model().checker().threads(num_cpus::get())
                .serve(address);
        }
        // Some("spawn") => {
        //     let port = 3000;
        //
        //     println!("  A set of servers that implement Single Decree Paxos.");
        //     println!("  You can monitor and interact using tcpdump and netcat.");
        //     println!("  Use `tcpdump -D` if you see error `lo0: No such device exists`.");
        //     println!("Examples:");
        //     println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
        //     println!("$ nc -u localhost {}", port);
        //     println!("{}", serde_json::to_string(&RegisterMsg::Put::<RequestId, Value, ()>(1, 'X')).unwrap());
        //     println!("{}", serde_json::to_string(&RegisterMsg::Get::<RequestId, Value, ()>(2)).unwrap());
        //     println!();
        //
        //     // WARNING: Omits `ordered_reliable_link` to keep the message
        //     //          protocol simple for `nc`.
        //     let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 0));
        //     let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
        //     let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
        //     spawn(
        //         serde_json::to_vec,
        //         |bytes| serde_json::from_slice(bytes),
        //         vec![
        //             (id0, PingerActor { peer_ids: vec![id1, id2] }),
        //             (id1, PingerActor { peer_ids: vec![id0, id2] }),
        //             (id2, PingerActor { peer_ids: vec![id0, id1] }),
        //         ]).unwrap();
        // }
        _ => {
            println!("USAGE:");
            // println!("  ./paxos check [CLIENT_COUNT] [NETWORK]");
            println!("  ./timers explore [CLIENT_COUNT] [ADDRESS] [NETWORK]");
            // println!("  ./paxos spawn");
            println!("NETWORK: {}", Network::<<PingerActor as Actor>::Msg>::names().join(" | "));
        }
    }

    Ok(())
}
