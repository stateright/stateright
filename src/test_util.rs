//! Utilities for tests.

/// A machine that cycles between two states.
pub mod binary_clock {
    use crate::*;

    #[derive(Clone)]
    pub struct BinaryClock;

    #[derive(Clone, Debug, PartialEq)]
    pub enum BinaryClockAction {
        GoLow,
        GoHigh,
    }

    pub type BinaryClockState = i8;

    impl Model for BinaryClock {
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
                BinaryClockAction::GoLow => Some(0),
                BinaryClockAction::GoHigh => Some(1),
            }
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![Property::always("in [0, 1]", |_, state| {
                0 <= *state && *state <= 1
            })]
        }
    }
}

/// A directed graph, specified via paths from initial states.
pub mod dgraph {
    use crate::Checker;
    use crate::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Clone)]
    pub struct DGraph {
        inits: BTreeSet<u8>,
        edges: BTreeMap<u8, BTreeSet<u8>>,
        property: Vec<Property<DGraph>>,
    }

    impl DGraph {
        pub fn with_properties(properties: Vec<Property<DGraph>>) -> Self {
            DGraph {
                inits: Default::default(),
                edges: Default::default(),
                property: properties,
            }
        }

        pub fn with_property(property: Property<DGraph>) -> Self {
            Self::with_properties(vec![property])
        }

        pub fn with_path(mut self, path: Vec<u8>) -> Self {
            let mut src = *path.first().unwrap();
            DGraph {
                inits: {
                    self.inits.insert(src);
                    self.inits
                },
                edges: {
                    for &dst in path.iter().skip(1) {
                        self.edges.entry(src).or_default().insert(dst);
                        src = dst;
                    }
                    self.edges
                },
                property: self.property,
            }
        }

        pub fn check(self) -> impl Checker<Self> {
            self.checker().spawn_bfs().join()
        }
    }

    impl Model for DGraph {
        type State = u8;
        type Action = u8;

        fn init_states(&self) -> Vec<Self::State> {
            self.inits.clone().into_iter().collect()
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            if let Some(dsts) = self.edges.get(state) {
                actions.extend(dsts);
            }
        }

        fn next_state(&self, _: &Self::State, action: Self::Action) -> Option<Self::State> {
            Some(action)
        }

        fn properties(&self) -> Vec<Property<Self>> {
            self.property.clone()
        }
    }
}

/// A model defined by a function.
pub mod function {
    use crate::*;

    impl<T: Clone> Model for fn(Option<&T>, &mut Vec<T>) {
        type State = T;
        type Action = T;
        fn init_states(&self) -> Vec<Self::State> {
            let mut actions = Default::default();
            self(None, &mut actions);
            actions
        }
        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            self(Some(state), actions);
        }
        fn next_state(&self, _: &Self::State, action: Self::Action) -> Option<Self::State> {
            Some(action)
        }
    }
}

/// A system that solves a linear Diophantine equation in `u8`.
pub mod linear_equation_solver {
    use crate::*;

    /// Given `a`, `b`, and `c`, finds `x` and `y` such that `a*x + b*y = c` where all values are
    /// in `u8`.
    pub struct LinearEquation {
        pub a: u8,
        pub b: u8,
        pub c: u8,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Guess {
        IncreaseX,
        IncreaseY,
    }

    impl Model for LinearEquation {
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

        fn properties(&self) -> Vec<Property<Self>> {
            vec![Property::sometimes("solvable", |equation, solution| {
                let LinearEquation { a, b, c } = equation;
                let (x, y) = solution;

                // dereference and enable wrapping so the equation is succinct
                use std::num::Wrapping;
                let (x, y) = (Wrapping(*x), Wrapping(*y));
                let (a, b, c) = (Wrapping(*a), Wrapping(*b), Wrapping(*c));

                a * x + b * y == c
            })]
        }
    }
}

/// A model that panicks during checking.
pub mod panicker {
    use crate::*;

    pub struct Panicker;

    impl Model for Panicker {
        type State = usize;

        type Action = usize;

        fn init_states(&self) -> Vec<Self::State> {
            vec![0]
        }

        fn actions(&self, _state: &Self::State, actions: &mut Vec<Self::Action>) {
            actions.push(1);
        }

        fn next_state(
            &self,
            last_state: &Self::State,
            action: Self::Action,
        ) -> Option<Self::State> {
            if *last_state == 5 {
                panic!("reached panic state");
            }
            Some(last_state + action)
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![Property::always("true", |_, _| true)]
        }
    }
}
