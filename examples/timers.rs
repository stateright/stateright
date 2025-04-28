use serde::{Deserialize, Serialize};
use stateright::actor::model_timeout;
use stateright::actor::{model_peers, Actor, ActorModel, Id, Network, Out};
use stateright::report::WriteReporter;
use stateright::{Checker, Model};
use std::borrow::Cow;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
enum PingerMsg {
    Ping,
    Pong,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
enum PingerTimer {
    Even,
    Odd,
    NoOp,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PingerState {
    sent: usize,
    received: usize,
}

#[derive(Clone)]
struct PingerActor {
    peer_ids: Vec<Id>,
}

impl Actor for PingerActor {
    type Msg = PingerMsg;
    type State = PingerState;
    type Timer = PingerTimer;
    type Random = ();
    type Storage = ();
    fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        o.set_timer(PingerTimer::Even, model_timeout());
        o.set_timer(PingerTimer::Odd, model_timeout());
        o.set_timer(PingerTimer::NoOp, model_timeout());
        PingerState {
            sent: 0,
            received: 0,
        }
    }

    fn on_msg(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        match msg {
            PingerMsg::Ping => {
                o.send(src, PingerMsg::Pong);
            }
            PingerMsg::Pong => {
                state.to_mut().received += 1;
            }
        }
    }

    fn on_timeout(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        match timer {
            PingerTimer::Even => {
                o.set_timer(PingerTimer::Even, model_timeout());
                for &dst in &self.peer_ids {
                    if usize::from(dst) % 2 == 0 {
                        state.to_mut().sent += 1;
                        o.send(dst, PingerMsg::Ping);
                    }
                }
            }
            PingerTimer::Odd => {
                o.set_timer(PingerTimer::Odd, model_timeout());
                for &dst in &self.peer_ids {
                    if usize::from(dst) % 2 != 0 {
                        state.to_mut().sent += 1;
                        o.send(dst, PingerMsg::Ping);
                    }
                }
            }
            PingerTimer::NoOp => {
                o.set_timer(PingerTimer::NoOp, model_timeout());
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
    fn into_model(self) -> ActorModel<PingerActor, Self, ()> {
        ActorModel::new(self.clone(), ())
            .actors((0..self.server_count).map(|i| PingerActor {
                peer_ids: model_peers(i, self.server_count),
            }))
            .init_network(self.network)
            .property(stateright::Expectation::Always, "true", |_, _| true)
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!("Model checking Pingers");
            PingerModelCfg {
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .spawn_dfs()
            .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!("Exploring state space for Pingers on {}.", address);
            PingerModelCfg {
                server_count: 3,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .serve(address);
        }
        _ => {
            println!("USAGE:");
            println!("  ./timers check [CLIENT_COUNT] [NETWORK]");
            println!("  ./timers explore [CLIENT_COUNT] [ADDRESS] [NETWORK]");
            println!(
                "NETWORK: {}",
                Network::<<PingerActor as Actor>::Msg>::names().join(" | ")
            );
        }
    }

    Ok(())
}
