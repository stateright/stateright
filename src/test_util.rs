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

        fn next_state(&self, _state: &Self::State, action: Self::Action) -> Option<Self::State> {
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

        fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
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

    pub enum PingPong { PingActor { pong_id: Id }, PongActor }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct PingPongCount(pub u32);

    #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum PingPongMsg { Ping(u32), Pong(u32) }

    impl Actor for PingPong {
        type Msg = PingPongMsg;
        type State = PingPongCount;

        fn init(i: InitIn<Self>, o: &mut Out<Self>) {
            o.set_state(PingPongCount(0));
            if let PingPong::PingActor { pong_id } = i.context {
                o.send(*pong_id, PingPongMsg::Ping(0));
            }
        }

        fn next(i: NextIn<Self>, o: &mut Out<Self>) {
            match i.event {
                Event::Receive(src, PingPongMsg::Pong(msg_value)) if i.state.0 == msg_value => {
                    if let PingPong::PingActor { .. } = i.context {
                        o.set_state(PingPongCount(i.state.0 + 1));
                        o.send(src, PingPongMsg::Ping(msg_value + 1));
                    }
                },
                Event::Receive(src, PingPongMsg::Ping(msg_value)) if i.state.0 == msg_value => {
                    if let PingPong::PongActor = i.context {
                        o.set_state(PingPongCount(i.state.0 + 1));
                        o.send(src, PingPongMsg::Pong(msg_value));
                    }
                },
                _ => {},
            }
        }
    }

    impl System<PingPong> {
        pub fn model(self, max_nat: u32) -> Model<'static, Self> {
            Model {
                state_machine: self,
                properties: vec![Property::always("delta within 1", |_sys, state: &SystemState<PingPong>| {
                    let max = state.actor_states.iter().map(|s| s.0).max().unwrap();
                    let min = state.actor_states.iter().map(|s| s.0).min().unwrap();
                    max - min <= 1
                })],
                boundary: Some(Box::new( move |_sys, state| {
                    state.actor_states.iter().all(|s| s.0 <= max_nat)
                })),
            }
        }
    }
}
