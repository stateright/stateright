use std::net::{Ipv4Addr, SocketAddrV4};

use serde::{Deserialize, Serialize};
use stateright::{
    actor::{spawn, Actor, ActorModel, Id},
    report::WriteReporter,
    util::HashableHashMap,
    CheckerBuilder, Model,
};

/// A Last Write Wins (LWW) register, implemented as a state-based CRDT.
/// <https://www.bartoszsypytkowski.com/the-state-of-a-state-based-crdts/#lastwritewinsregister>.
#[derive(Hash, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
struct LwwRegister<T> {
    value: T,
    timestamp: u128,
    updater_id: usize,
}

impl<T: Clone> LwwRegister<T> {
    fn merge(a: &Self, b: &Self) -> Self {
        if (a.timestamp, a.updater_id) > (b.timestamp, b.updater_id) {
            a.clone()
        } else {
            b.clone()
        }
    }

    fn set(&mut self, value: T, timestamp: u128, updater_id: usize) {
        self.value = value;
        self.timestamp = timestamp;
        self.updater_id = updater_id;
    }
}

#[derive(Hash, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
enum Value {
    A,
    B,
    C,
}

#[derive(Hash, Debug, PartialEq, Eq, Clone)]
enum LwwActorActions<T> {
    SetValue(T),
    SetTime(u128),
}

#[derive(Hash, Debug, PartialEq, Eq, Clone)]
struct LwwActorState {
    register: Option<LwwRegister<Value>>,
    local_clock: u128,
    maximum_used_clock: u128,
}

impl LwwActorState {
    fn new() -> Self {
        Self {
            register: None,
            local_clock: 1000,
            maximum_used_clock: 1000,
        }
    }
}

#[derive(Clone)]
struct LwwActor {
    peers: Vec<Id>,
}

impl LwwActor {
    fn populate_choices(&self, o: &mut stateright::actor::Out<Self>, time: u128) {
        o.choose_random(
            "node_action",
            vec![
                LwwActorActions::SetValue(Value::A),
                LwwActorActions::SetValue(Value::B),
                LwwActorActions::SetValue(Value::C),
                LwwActorActions::SetTime(time.saturating_add(1)),
                LwwActorActions::SetTime(time.saturating_sub(1)),
            ],
        );
    }
}

impl Actor for LwwActor {
    type Msg = LwwRegister<Value>;
    type Timer = ();
    type State = LwwActorState;
    type Random = LwwActorActions<Value>;
    type Storage = ();

    fn on_start(
        &self,
        _: stateright::actor::Id,
        _: &Option<Self::Storage>,
        o: &mut stateright::actor::Out<Self>,
    ) -> Self::State {
        let state = LwwActorState::new();
        self.populate_choices(o, state.local_clock);
        state
    }

    fn on_random(
        &self,
        id: stateright::actor::Id,
        state: &mut std::borrow::Cow<Self::State>,
        random: &Self::Random,
        o: &mut stateright::actor::Out<Self>,
    ) {
        match random {
            LwwActorActions::SetValue(value) => {
                let state_mut = state.to_mut();
                if let Some(register) = &mut state_mut.register {
                    // Ensure clock value is unique per node
                    let clock_value = state_mut.local_clock.max(state_mut.maximum_used_clock + 1);
                    register.set(value.clone(), clock_value, usize::from(id));
                    state_mut.maximum_used_clock = clock_value;
                } else {
                    state_mut.register = Some(LwwRegister {
                        value: value.clone(),
                        timestamp: state_mut.local_clock,
                        updater_id: usize::from(id),
                    })
                }
                o.broadcast(self.peers.iter(), state_mut.register.as_ref().unwrap());
            }
            LwwActorActions::SetTime(time) => {
                state.to_mut().local_clock = *time;
            }
        }
        self.populate_choices(o, state.local_clock)
    }

    fn on_msg(
        &self,
        _: Id,
        state: &mut std::borrow::Cow<Self::State>,
        _: Id,
        msg: Self::Msg,
        _: &mut stateright::actor::Out<Self>,
    ) {
        let state_mut = state.to_mut();
        if let Some(register) = &mut state_mut.register {
            let merged = LwwRegister::merge(register, &msg);
            *register = merged;
        } else {
            state_mut.register = Some(msg.clone())
        }
    }
}

fn build_checker(num_actors: usize) -> CheckerBuilder<ActorModel<LwwActor>> {
    let nodes: Vec<_> = (0..num_actors).map(Id::from).collect();
    let mut checker_builder = ActorModel::new((), ());
    for _ in 0..num_actors {
        checker_builder = checker_builder.actor(LwwActor {
            peers: nodes.clone(),
        });
    }
    checker_builder
        .init_network(stateright::actor::Network::UnorderedNonDuplicating(
            HashableHashMap::new(),
        ))
        .property(
            stateright::Expectation::Always,
            "eventually consistent",
            |_, state| {
                // CRDT's "eventual consistency" means the states should be consistent if there are
                // no more messages on the network. Unlike `Expectation::Eventually`, it doesn't
                // count if the condition was transiently satisfied before reaching the terminal
                // state.
                if state.network.len() == 0 {
                    let mut peekable_iter = state.actor_states.iter().peekable();
                    while let (Some(s1), Some(s2)) = (peekable_iter.next(), peekable_iter.peek()) {
                        if s1.register != s2.register {
                            return false;
                        }
                    }
                }
                true
            },
        )
        .checker()
}

pub fn main() -> Result<(), pico_args::Error> {
    use stateright::Checker;

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let depth = args.opt_free_from_str()?.unwrap_or(8);
            build_checker(client_count)
                .target_max_depth(depth)
                .spawn_dfs()
                .join_and_report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let client_count = args.opt_free_from_str()?.unwrap_or(2);
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            println!(
                "Exploring state space for last-writer-wins register with {} clients on {}.",
                client_count, address
            );
            build_checker(client_count).serve(address);
        }
        Some("spawn") => {
            let port = 3000;

            println!("  A server that implements a last-writer-wins register.");
            println!("  You can monitor and interact using tcpdump and netcat.");
            println!("  Use `tcpdump -D` if you see error `lo0: No such device exists`.");
            println!("Examples:");
            println!("$ sudo tcpdump -i lo0 -s 0 -nnX");
            println!("$ nc -u localhost {}", port);
            println!();
            println!("  This will run indefinitely to explore the state space.");
            println!();

            let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
            let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 1));
            let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port + 2));
            spawn(
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                serde_json::to_vec,
                |bytes| serde_json::from_slice(bytes),
                vec![
                    (
                        id0,
                        LwwActor {
                            peers: vec![id1, id2],
                        },
                    ),
                    (
                        id1,
                        LwwActor {
                            peers: vec![id0, id2],
                        },
                    ),
                    (
                        id2,
                        LwwActor {
                            peers: vec![id0, id1],
                        },
                    ),
                ],
            )
            .unwrap();
        }
        _ => {
            println!("USAGE:");
            println!("  ./lww-register check [CLIENT_COUNT] [DEPTH]");
            println!("  ./lww-register explore [CLIENT_COUNT] [ADDRESS]");
            println!("  ./lww-register spawn");
        }
    }
    Ok(())
}
