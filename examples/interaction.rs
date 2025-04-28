// This module exemplifies how to use an Actor to model user interaction or other external inputs
// for a system in which the states of the actors do not evolve autonomously.

use std::borrow::Cow;

use choice::{choice, Choice};
use stateright::actor::{model_timeout, Actor, ActorModel, Id, Out};
use stateright::{report::WriteReporter, Checker, Expectation, Model};

use std::fmt::Debug;
use std::hash::Hash;

use serde::{Deserialize, Serialize};

use std::sync::Arc;

fn main() -> Result<(), pico_args::Error> {
    let mut args = pico_args::Arguments::from_env();

    let model = ActorModel::<choice![Client, Counter], (), u8>::new((), 0)
        .actor(Choice::new(Client {
            threshold: 3,
            counter_addr: 1.into(),
        }))
        .actor(
            Choice::new(Counter {
                initial_state: CounterState {
                    addr: 1.into(),
                    counter: 0,
                },
            })
            .or(),
        );

    match args.subcommand()?.as_deref() {
        Some("check") => {
            let checker = model
                .property(Expectation::Eventually, "success", |_, state| {
                    state.actor_states.iter().any(state_filter_success)
                })
                .checker()
                .threads(num_cpus::get())
                .target_max_depth(30) // To ensure termination of the checker, since the state space is very loosely bounded.
                .spawn_bfs()
                .report(&mut WriteReporter::new(&mut std::io::stdout()));

            checker.assert_properties();
        }

        Some("explore") => {
            let checker = model
                .property(Expectation::Eventually, "success", |_, state| {
                    state.actor_states.iter().any(state_filter_success)
                })
                .checker()
                .threads(num_cpus::get())
                .target_max_depth(30); // To ensure termination of the checker, since the state space is very loosely bounded.

            checker.serve("0:3000");
        }

        _ => {
            println!("USAGE:");
            println!("  ./interaction check");
            println!("  ./interaction explore");
        }
    }
    Ok(())
}

// We use this helper function to improve readability of check_discovery.
// It also shows how to use the choice! macro for destructuring.
fn state_filter_success(s: &Arc<choice![InputState, CounterState]>) -> bool {
    match s.as_ref() {
        choice!(0 -> v) => v.success, // Client
        choice!(1 -> _v) => false,    // CounterState
        choice!(2 -> !) => false,     // Never
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Msg {
    IncrementRequest(CounterSize),
    ReportRequest(),
    ReplyCount(CounterSize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Counter {
    pub initial_state: CounterState,
}

pub type CounterSize = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CounterState {
    pub addr: Id,
    pub counter: CounterSize,
}

impl Actor for Counter {
    type Msg = Msg;
    type State = CounterState;
    type Timer = InputTimer;
    type Random = ();
    type Storage = ();
    fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, _o: &mut Out<Self>) -> Self::State {
        self.initial_state
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
            Self::Msg::IncrementRequest(n) => {
                *state.to_mut() = CounterState {
                    counter: state.counter + n,
                    ..**state
                }
            }

            Self::Msg::ReportRequest() => o.send(src, Self::Msg::ReplyCount(state.counter)),

            _ => (),
        };

        //*state.to_mut() = new_state;
    }
}

pub struct Client {
    pub threshold: CounterSize, // The number the Client wants the counter to report in the end.
    pub counter_addr: Id,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputState {
    pub wait_cycles: u32, // Only used for observing system evolution in the explorer.
    pub success: bool,
}

// Timers are discrete and time out immediately for model checking purposes.
// To trigger actions in certain orders they need to be set by the correct events.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum InputTimer {
    ClientInput,
    ClientQuery,
}

impl Actor for Client {
    type Msg = Msg;
    type State = InputState;
    type Timer = InputTimer;
    type Random = ();
    type Storage = ();

    fn on_start(&self, _id: Id, _storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        // Set a timeout to trigger sending increment request.
        o.set_timer(InputTimer::ClientInput, model_timeout());
        InputState {
            wait_cycles: 0,
            success: false,
        }
    }

    fn on_msg(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        _src: Id,
        msg: Self::Msg,
        _o: &mut Out<Self>,
    ) {
        if let Msg::ReplyCount(n) = msg {
            if n >= self.threshold {
                state.to_mut().success = true;
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
            InputTimer::ClientInput => {
                // Set timeout for requesting success status s.t. it happens after incrementing.
                o.set_timer(InputTimer::ClientQuery, model_timeout());
                o.send(self.counter_addr, Msg::IncrementRequest(3));
                state.to_mut().wait_cycles += 1; // Increment InputCycles
            }
            InputTimer::ClientQuery => {
                o.send(self.counter_addr, Msg::ReportRequest());
                state.to_mut().wait_cycles += 1;
            }
        }
    }
}
