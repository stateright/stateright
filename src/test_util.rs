//! Utilities for tests.

use crate::StateMachine;

/// A machine that cycles between two states.
pub mod binary_clock {
    use super::*;

    pub struct BinaryClock;

    #[derive(Clone, Debug, PartialEq)]
    pub enum BinaryClockAction { GoLow, GoHigh }

    pub type BinaryClockState = u8;

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
}

