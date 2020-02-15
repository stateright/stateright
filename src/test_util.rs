//! Utilities for tests.

/// A machine that cycles between two states.
pub mod binary_clock {
    use crate::*;
    use crate::checker::*;

    #[derive(Clone)]
    pub struct BinaryClock;

    #[derive(Clone, Debug, PartialEq)]
    pub enum BinaryClockAction { GoLow, GoHigh }

    pub type BinaryClockState = i8;

    impl StateMachine for BinaryClock {
        type State = BinaryClockState;
        type Action = BinaryClockAction;

        fn init_states(&self) -> Vec<Self::State> {
            vec![0, 1]
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            if *state == 0 {
                actions.push(BinaryClockAction::GoHigh);
            } else {
                actions.push(BinaryClockAction::GoLow);
            }
        }

        fn next_state(&self, _state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match action {
                BinaryClockAction::GoLow  => Some(0),
                BinaryClockAction::GoHigh => Some(1),
            }
        }
    }

    impl BinaryClock {
        pub fn model(self) -> Model<'static, Self> {
            Model {
                state_machine: self,
                properties: vec![Property::always("in [0, 1]", |_model, state| {
                    0 <= *state && *state <= 1
                })],
                boundary: None,
            }
        }
    }
}

/// A state machine that solves linear equations in two dimensions.
pub mod linear_equation_solver {
    use crate::*;
    use crate::checker::*;

    /// Given `a`, `b`, and `c`, finds `x` and `y` such that `a*x + b*y = c` where all values are
    /// in `u8`.
    pub struct LinearEquation { pub a: u8, pub b: u8, pub c: u8 }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Guess { IncreaseX, IncreaseY }

    impl StateMachine for LinearEquation {
        type State = (u8, u8);
        type Action = Guess;

        fn init_states(&self) -> Vec<Self::State> {
            vec![(0, 0)]
        }

        fn actions(&self, _state: &Self::State, actions: &mut Vec<Self::Action>) {
            actions.push(Guess::IncreaseX);
            actions.push(Guess::IncreaseY);
        }

        fn next_state(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            let (x, y) = *state;
            match &action {
                Guess::IncreaseX => Some((x.wrapping_add(1), y)),
                Guess::IncreaseY => Some((x, y.wrapping_add(1))),
            }
        }
    }

    impl LinearEquation {
        pub fn model(self) -> Model<'static, Self> {
            Model {
                state_machine: self,
                properties: vec![Property::sometimes("solvable", |equation, solution| {
                    let LinearEquation { a, b, c } = equation;
                    let (x, y) = solution;

                    // dereference and enable wrapping so the equation is succinct
                    use std::num::Wrapping;
                    let (x, y) = (Wrapping(*x), Wrapping(*y));
                    let (a, b, c) = (Wrapping(*a), Wrapping(*b), Wrapping(*c));

                    a*x + b*y == c
                })],
                boundary: None,
            }
        }
    }
}

/// A pair of actors that send messages and increment message counters.
pub mod ping_pong {
    use crate::*;
    use crate::actor::*;
    use crate::actor::system::*;
    use crate::checker::*;

    pub enum PingPong {
        Pinger { max_nat: u32, ponger_id: Id },
        Ponger { max_nat: u32 }
    }

    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum State { Pinger(u32), Ponger(u32) }

    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum Msg { Ping(u32), Pong(u32) }

    impl Actor for PingPong {
        type Msg = Msg;
        type State = State;

        fn start(&self) -> ActorResult<Self::Msg, Self::State> {
            match self {
                PingPong::Pinger { ponger_id, .. } => ActorResult::start(
                    State::Pinger(0),
                    |outputs| outputs.send(*ponger_id, Msg::Ping(0))),
                PingPong::Ponger { .. } => ActorResult::start(
                    State::Ponger(0),
                    |_outputs| {}),
            }
        }

        fn advance(&self, state: &Self::State, input: &ActorInput<Self::Msg>) -> Option<ActorResult<Self::Msg, Self::State>> {
            let ActorInput::Deliver { src, msg } = input.clone();
            match self {
                &PingPong::Pinger { max_nat, .. } => {
                    if let &State::Pinger(actor_value) = state {
                        if let Msg::Pong(msg_value) = msg {
                            if actor_value == msg_value && actor_value < max_nat {
                                return ActorResult::advance(state, |state, outputs| {
                                    *state = State::Pinger(actor_value + 1);
                                    outputs.send(src, Msg::Ping(msg_value + 1));
                                });
                            }
                        }
                    }
                    return None;
                }
                &PingPong::Ponger { max_nat, .. } => {
                    if let &State::Ponger(actor_value) = state {
                        if let Msg::Ping(msg_value) = msg {
                            if actor_value == msg_value && actor_value < max_nat {
                                return ActorResult::advance(state, |state, outputs| {
                                    *state = State::Ponger(actor_value + 1);
                                    outputs.send(src, Msg::Pong(msg_value));
                                });
                            }
                        }
                    }
                    return None;
                }
            }
        }
    }

    impl ActorSystem<PingPong> {
        pub fn model(self) -> Model<'static, Self> {
            Model {
                state_machine: self,
                properties: vec![Property::always("delta within 1", |_sys, snap| {
                    use std::sync::Arc;

                    let &ActorSystemSnapshot { ref actor_states, .. } = snap;
                    fn extract_value(a: &Arc<State>) -> u32 {
                        match **a {
                            State::Pinger(value) => value,
                            State::Ponger(value) => value,
                        }
                    };

                    let max = actor_states.iter().map(extract_value).max().unwrap();
                    let min = actor_states.iter().map(extract_value).min().unwrap();
                    max - min <= 1
                })],
                boundary: None, // the actors already have max_nat
            }
        }
    }
}
