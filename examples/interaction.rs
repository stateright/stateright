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
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let checker =
                ActorModel::<choice![ExternalInput, Supervisor, Counter], (), u8>::new((), 0)
                    .actor(Choice::new(ExternalInput {
                        threshold: 3,
                        supervisor_addr: 1.into(),
                    }))
                    .actor(
                        Choice::new(Supervisor {
                            initial_state: SupervisorState {
                                addr: 1.into(),
                                threshold: 3,
                                counter_addr: 2.into(),
                                input_addr: 0.into(),
                                success: false,
                            },
                        })
                        .or(),
                    )
                    .actor(
                        Choice::new(Counter {
                            initial_state: CounterState {
                                addr: 1.into(),
                                counter: 0,
                            },
                        })
                        .or()
                        .or(),
                    )
                    .property(Expectation::Eventually, "success", |_, state| {
                        state.actor_states.iter().any(|s| state_filter_success(s))
                    })
                    .checker()
                    .threads(num_cpus::get())
                    .target_max_depth(30) // To ensure termination of the checker, since the state space is very loosely bounded.
                    .spawn_bfs()
                    .report(&mut WriteReporter::new(&mut std::io::stdout()));

            checker.assert_properties();
        }

        Some("explore") => {
            let checker =
                ActorModel::<choice![ExternalInput, Supervisor, Counter], (), u8>::new((), 0)
                    .actor(Choice::new(ExternalInput {
                        threshold: 3,
                        supervisor_addr: 1.into(),
                    }))
                    .actor(
                        Choice::new(Supervisor {
                            initial_state: SupervisorState {
                                addr: 1.into(),
                                threshold: 3,
                                counter_addr: 2.into(),
                                input_addr: 0.into(),
                                success: false,
                            },
                        })
                        .or(),
                    )
                    .actor(
                        Choice::new(Counter {
                            initial_state: CounterState {
                                addr: 1.into(),
                                counter: 0,
                            },
                        })
                        .or()
                        .or(),
                    )
                    .property(Expectation::Eventually, "success", |_, state| {
                        state.actor_states.iter().any(|s| state_filter_success(s))
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
fn state_filter_success(s: &Arc<choice![InputState, SupervisorState, CounterState]>) -> bool {
    match s.as_ref() {
        choice!(0 -> _v) => false, // InputState
        choice!(1 -> v) => {
            if v.success == true {
                true
            } else {
                false
            }
        } // SupervisorState
        choice!(2 -> _v) => false, // CounterState
        choice!(3 -> !) => false,  // Never
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Msg {
    SupervisorIncrementRequest(CounterSize),
    SupervisorReportRequest(),
    CounterReplyCount(CounterSize),
}

// We use a Generic BaseActor to pass State to on_start, but Actors could be built using independent structs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseActor<T> {
    pub initial_state: T, //T needs to be a State Type
}

pub type CounterSize = u32;

pub type Counter = BaseActor<CounterState>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CounterState {
    pub addr: Id,
    pub counter: CounterSize,
}

impl Actor for Counter {
    type Msg = Msg;
    type State = CounterState;
    type Timer = InputTimer;

    fn on_start(&self, _id: Id, _o: &mut Out<Self>) -> Self::State {
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
        let (new_state, output_msgs) = match msg {
            Self::Msg::SupervisorIncrementRequest(n) => (
                CounterState {
                    counter: state.counter + n,
                    ..**state
                },
                Vec::new(),
            ),

            Self::Msg::SupervisorReportRequest() => (
                **state,
                vec![(src, Self::Msg::CounterReplyCount(state.counter))],
            ),

            _ => (**state, Vec::new()),
        };

        *state.to_mut() = new_state;

        for (addr, msg) in output_msgs {
            o.send(addr, msg)
        }
    }
}

// Note: The counter is separated into two machines to test message based interaction without any timers.
pub type Supervisor = BaseActor<SupervisorState>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SupervisorState {
    pub addr: Id,
    pub threshold: CounterSize,
    pub counter_addr: Id,
    pub input_addr: Id,
    pub success: bool,
}

impl Actor for Supervisor {
    type Msg = Msg;
    type State = SupervisorState;
    type Timer = InputTimer;

    fn on_start(&self, _id: Id, _o: &mut Out<Self>) -> Self::State {
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
        // InputActor triggers message forwarding.
        if src == state.input_addr {
            o.send(state.counter_addr, msg)
        } else {
            let (new_state, output_msgs) = match msg {
                Self::Msg::CounterReplyCount(n) => {
                    if n >= state.threshold {
                        (
                            SupervisorState {
                                success: true,
                                ..**state
                            },
                            Vec::new(),
                        )
                    } else {
                        (**state, Vec::new())
                    }
                }

                _ => (**state, Vec::new()),
            };

            *state.to_mut() = new_state;

            for (addr, msg) in output_msgs {
                o.send(addr, msg)
            }
        }
    }
}

pub struct ExternalInput {
    pub threshold: CounterSize,
    pub supervisor_addr: Id,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputState {
    pub wait_cycles: u32, // Only used for observing system evolution in the explorer.
    pub done: bool,
}

// Timers are discrete and time out immediately for model checking purposes.
// To trigger actions in certain orders they need to be set by the correct events.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum InputTimer {
    RequestIncrement,
    RequestSuccess,
}

impl Actor for ExternalInput {
    type Msg = Msg;
    type State = InputState;
    type Timer = InputTimer;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) -> Self::State {
        // Set a timeout to trigger sending increment request.
        o.set_timer(InputTimer::RequestIncrement, model_timeout());
        InputState {
            wait_cycles: 0,
            done: false,
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
        match msg {
            Msg::CounterReplyCount(n) => {
                if n >= self.threshold {
                    state.to_mut().done = true;
                }
            }
            _ => (),
        }
    }

    fn on_timeout(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        // We use the reuse the message types of SupervisorMachine here to simulate a user triggering Supervisor behavior.
        match timer {
            InputTimer::RequestIncrement => {
                // Set timeout for requesting success status s.t. it happens after incrementing.
                o.set_timer(InputTimer::RequestSuccess, model_timeout());
                o.send(self.supervisor_addr, Msg::SupervisorIncrementRequest(3));
                state.to_mut().wait_cycles += 1; // Increment InputCycles
            }
            InputTimer::RequestSuccess => {
                o.send(self.supervisor_addr, Msg::SupervisorReportRequest());
                state.to_mut().wait_cycles += 1;
            }
        }
    }
}
