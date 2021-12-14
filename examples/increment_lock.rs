use itertools::*;
use stateright::*;

#[derive(Debug, Clone)]
pub enum Action {
    Lock(usize),
    Read(usize),
    Write(usize),
    Release(usize),
}

#[derive(Debug, Clone, Default, Hash, Eq, Ord, PartialEq, PartialOrd)]
struct ProcState {
    /// Thread_local state.
    t: u8,
    /// Program counter.
    pc: u8,
}

#[derive(Debug, Clone, Default, Hash, PartialEq)]
pub struct State {
    i: u8,
    lock: bool,
    s: Vec<ProcState>,
}

impl State {
    pub fn new(n: usize) -> Self {
        Self {
            i: 0,
            lock: false,
            s: vec![ProcState { t: 0, pc: 0 }; n],
        }
    }
}

impl Symmetric for State {
    fn permutations(&self) -> Box<dyn Iterator<Item = Self>> {
        let this = self.clone();
        Box::new((0..self.s.len())
            .permutations(self.s.len())
            .map(move |pi| {
                let this = &this;
                Self {
                    i : this.i,
                    lock : this.lock,
                    s : pi.iter().map(|&i| this.s[i].clone()).collect(),
                }
            }))
    }

    fn a_sorted_permutation(&self) -> Self {
        let mut main_array = self.s.clone();
        main_array.sort();
        Self {
            i : self.i,
            lock : self.lock,
            s : main_array,
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
                0 if !state.lock => actions.push(Action::Lock(thread_id)),
                1 => actions.push(Action::Read(thread_id)),
                2 => actions.push(Action::Write(thread_id)),
                3 if state.lock => actions.push(Action::Release(thread_id)),
                _ => {}
            }
        }
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            Action::Lock(n) => {
                let mut state = last_state.clone();
                state.s[n].pc = 1;
                state.lock = true;
                Some(state)
            }
            Action::Read(n) => {
                let mut state = last_state.clone();
                state.s[n].pc = 2;
                state.s[n].t = last_state.i;
                Some(state)
            }
            Action::Write(n) => {
                let mut state = last_state.clone();
                state.s[n].pc = 3;
                state.i = last_state.s[n].t + 1;
                Some(state)
            }
            Action::Release(n) => {
                let mut state = last_state.clone();
                state.s[n].pc = 4;
                state.lock = false;
                Some(state)
            }
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::always("fin", |_, state| {
                state.s.iter().filter(|&s| s.pc >= 3).count() as u8 == state.i
            }),
            Property::<Self>::always("mutex", |_, state| {
                state.s.iter().filter(|&s| s.pc >= 1 && s.pc < 4).count() <= 1
            }),
        ]
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            println!("Model checking increment with {} threads.", thread_count);

            State::new(thread_count)
                .checker()
                .threads(num_cpus::get())
                .spawn_dfs()
                .report(&mut std::io::stdout());
        }
        Some("check-sym") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            let strategy = args.opt_free_from_fn(parse_strategy)?.unwrap_or(Strategy::Full);
            println!(
                "Symmetrical model checking using {:?} increment with {} threads.",
                strategy,
                thread_count
            );


            State::new(thread_count)
                .checker()
                .threads(num_cpus::get())
                .spawn_sym(strategy)
                .report(&mut std::io::stdout());
        }
        Some("explore") => {
            let thread_count = args.opt_free_from_str()?.unwrap_or(3);
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            println!(
                "Exploring the state space of increment with {} threads on {}.",
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
            println!("  ./general_increment explore [THREAD_COUNT] [ADDRESS]");
        }
    }

    Ok(())
}

fn parse_strategy(s : &str) -> Result<Strategy, &'static str> {
   print!("{}", s);
   match s {
       "full" => Ok(Strategy::Full),
       "sorted" => Ok(Strategy::Sorted),
       _ => Err("Only valid for full/sorted")
   }
}
