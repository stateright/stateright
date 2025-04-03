//! # This Example
//!
//! This example illustrates a multithreaded system analogous to the following pseudocode (where
//! numeric labels inside the `spawn` command indicate atomic instructions):
//!
//! ```
//! SHARED = 0;
//! for _ in 0..N {
//!     spawn({
//!            let thread_local = 0;
//!         1: thread_local = SHARED;
//!         2: SHARED = thread_local + 1;
//!         3:
//!     })
//! }
//! ```
//!
//! # A Possible Intended Invariant
//!
//! If each thread was run in sequence, we would maintain an invariant whereby `SHARED`
//! always equals the number of threads that have finished executing atomic instruction #2. Since
//! the threads interleave, the final value can be smaller. For example, with two threads:
//!
//! ```
//! - Read(1)  // thread #1 reads 0 from shared memory into local memory
//! - Read(0)  // thread #0 reads 0 from shared memory into local memory
//! - Write(1) // thread #1 finishes by writing 1 into shared memory, maintaining the invariant
//! - Write(0) // thread #0 finishes by rewriting 1 into shared memory, breaking the invariant
//! ```
//!
//! # State Space Without Symmetry Reduction
//!
//! In the rest of this explanation, to match the implementation we use `i` to represent `SHARED`,
//! `ts[_]` to represent each `thread_local`, and `pcs[_]` to represent each program counter.
//!
//! Without symmetry reduction, the state space is composed of 13 unique states:
//!
//! ```
//! // 1. The system has a deterministic initial state.
//! State { i: 0, s: [{t: 0, pc: 1}, {t: 0, pc: 1}]}
//!
//! // 2. Next one of the threads reads from shared memory, resulting in two possible states.
//! State { i: 0, s: [{t: 0, pc: 2}, {t: 0, pc: 1}]}
//! State { i: 0, s: [{t: 0, pc: 1}, {t: 0, pc: 2}]}
//!
//! // 3a. Then either the same thread writes to shared memory...
//! State { i: 1, s: [{t: 0, pc: 1}, {t: 0, pc: 3}]}
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 1}]}
//!
//! // 3b. ... or the other thread catches up by also reading from shared memory.
//! State { i: 0, s: [{t: 0, pc: 2}, {t: 0, pc: 2}]}
//!
//! // 4a. In the case where the same thread wrote to shared memory, the other thread must observe
//! //     that write and will then must persist the increment, maintaining the invariant.
//! //
//! //     The read:
//! State { i: 1, s: [{t: 1, pc: 2}, {t: 0, pc: 3}]}
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 1, pc: 2}]}
//! //     The write:
//! State { i: 2, s: [{t: 1, pc: 3}, {t: 0, pc: 3}]}
//! State { i: 2, s: [{t: 0, pc: 3}, {t: 1, pc: 3}]}
//!
//! // 4b. Otherwise (in the case where both threads read the original shared memory), one writes
//! //     the increment (maintaining the invariant) before the other writes the stale increment
//! //     (breaking the invariant).
//! //
//! //     First write:
//! State { i: 1, s: [{t: 0, pc: 2}, {t: 0, pc: 3}]}
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 2}]}
//! //     Second write:
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 3}]}
//! ```
//!
//! # State Space With Symmetry Reduction
//!
//! All threads are identical, so symmetry reduction can be employed to reduce the state space from
//! 13 to 8, eliminating 5 states:
//!
//! ```
//! // 1. Same as without symmetry reduction.
//! State { i: 0, s: [{t: 0, pc: 1}, {t: 0, pc: 1}]}
//!
//! // 2. Reduction eliminates 1 state.
//! State { i: 0, s: [{t: 0, pc: 2}, {t: 0, pc: 1}]}
//!
//! // 3a. Reduction eliminates 1 state.
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 1}]}
//!
//! // 3b. Same as without symmetry reduction.
//! State { i: 1, s: [{t: 0, pc: 2}, {t: 0, pc: 2}]}
//!
//! // 4a. Reduction eliminates 2 states.
//! //
//! //     Read:
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 1, pc: 2}]}
//! //     Write:
//! State { i: 2, s: [{t: 0, pc: 3}, {t: 1, pc: 3}]}
//!
//! // 4b. Reduction eliminates 1 state.
//! //
//! //     First write:
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 2}]}
//! //     Second write:
//! State { i: 1, s: [{t: 0, pc: 3}, {t: 0, pc: 3}]}
//! ```

use stateright::{report::WriteReporter, *};

#[derive(Debug, Clone)]
pub enum Action {
    /// A specified thread reads from the shared state into its local state.
    Read(usize),
    /// A specified thread writes its local state to the shared state.
    Write(usize),
}

#[derive(Debug, Clone, Default, Hash, Eq, Ord, PartialEq, PartialOrd)]
struct ProcState {
    /// Thread_local state.
    t: u8,
    /// Program counter.
    pc: u8,
}

#[derive(Debug, Clone, Default, Hash)]
pub struct State {
    /// The shared global state.
    i: u8,
    /// Each thread's internal state.
    s: Vec<ProcState>,
}

impl State {
    pub fn new(n: usize) -> Self {
        Self {
            i: 0,
            s: vec![ProcState { t: 0, pc: 1 }; n],
        }
    }
}

impl Representative for State {
    fn representative(&self) -> Self {
        let mut main_array = self.s.clone();
        main_array.sort();
        Self {
            i: self.i,
            s: main_array,
        }
    }
}

impl Model for State {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> std::vec::Vec<<Self as stateright::Model>::State> {
        vec![self.clone()]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for thread_id in 0..self.s.len() {
            match state.s[thread_id].pc {
                1 => actions.push(Action::Read(thread_id)),
                2 => actions.push(Action::Write(thread_id)),
                _ => {}
            }
        }
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            Action::Read(n) => {
                // Read the shared state into the specified thread's local state.
                let mut state = last_state.clone();
                state.s[n] = ProcState {
                    pc: 2,
                    t: last_state.i,
                };
                Some(state)
            }
            Action::Write(n) => {
                // Write the increment of the specified thread's local state to the shared state.
                let mut state = last_state.clone();
                state.s[n].pc = 3;
                state.i = last_state.s[n].t + 1;
                Some(state)
            }
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![Property::<Self>::always("fin", |_, state| {
            state.s.iter().filter(|&s| s.pc == 3).count() as u8 == state.i
        })]
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            println!(
                "Model checking increment_lock with {} threads.",
                thread_count
            );

            State::new(thread_count)
                .checker()
                .threads(num_cpus::get())
                .spawn_dfs()
                .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("check-sym") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            println!(
                "Model checking increment_lock with {} threads using symmetry reduction.",
                thread_count
            );

            State::new(thread_count)
                .checker()
                .threads(num_cpus::get())
                .symmetry()
                .spawn_dfs()
                .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            println!(
                "Exploring the state space of increment_lock with {} threads on {}.",
                thread_count, address
            );
            State::new(thread_count)
                .checker()
                .threads(num_cpus::get())
                .serve(address);
        }
        _ => {
            println!("USAGE:");
            println!("  ./increment check [THREAD_COUNT]");
            println!("  ./increment check-sym [THREAD_COUNT] [full|sorted]");
            println!("  ./increment explore [THREAD_COUNT] [ADDRESS]");
        }
    }

    Ok(())
}
