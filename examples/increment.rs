use stateright::*;
use itertools::*;

#[derive(Debug, Clone)]
pub enum Action {
    Read(usize),
    Write(usize),
}

#[derive(Debug, Clone, Default, Hash)]
pub struct State {
    i: u8,
    ts: Vec<u8>,
    pcs: Vec<u8>,
}


impl State {
    pub fn new(n: usize) -> Self {
        Self {
            i: 0,
            ts: vec![0; n],
            pcs: vec![1; n],
        }
    }

}

type Permutation = Vec<usize>;

impl Symmetric for State {
    type Permutation = Permutation;

    fn permute(&self, pi : &Permutation) -> Self {
        let tsp = pi.iter().map(|&i| self.ts[i]).collect();
        let pcsp = pi.iter().map(|&i| self.pcs[i]).collect();

        Self {
            i : self.i,
            ts : tsp,
            pcs : pcsp,
        }
    }

    fn get_permutations(&self) -> Vec<Permutation> {
        (0..self.ts.len())
            .permutations(self.ts.len())
            .collect()
    }
}

impl Model for State {
    type State = State;
    type Action = Action;


    fn init_states(&self) -> std::vec::Vec<<Self as stateright::Model>::State> {
        vec![self.clone()]
    }

    fn actions(&self, _state: &Self::State, actions: &mut Vec<Self::Action>) {
        actions.extend(&mut (0..self.pcs.len()).map(Action::Read));
        actions.extend(&mut (0..self.pcs.len()).map(Action::Write));
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        match action {
            Action::Read(n) => {
                if last_state.pcs[n] == 1 {
                    let mut state = last_state.clone();
                    state.ts[n] = last_state.i;
                    state.pcs[n] = 2;
                    Some(state)
                } else {
                    None
                }
            }
            Action::Write(n) => {
                if last_state.pcs[n] == 2 {
                    let mut state = last_state.clone();
                    state.pcs[n] = 3;
                    state.i = last_state.ts[n] + 1;
                    Some(state)
                } else {
                    None
                }
            }
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![Property::<Self>::always("fin", |_, state| {
            state.pcs.iter().filter(|&pc| *pc == (3 as u8)).count() as u8 == state.i
        })]
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let thread_count = args.opt_free_from_str()?
                .unwrap_or(3);
            println!("Model checking increment with {} threads.",
                     thread_count);

            State::new(thread_count)
                .checker().threads(num_cpus::get())
                .spawn_dfs().report(&mut std::io::stdout());
        }
        Some("check-sym") => {
            let thread_count = args.opt_free_from_str()?
                .unwrap_or(3);
            println!("Symmetrical model checking increment with {} threads.",
                     thread_count);

            State::new(thread_count)
                .checker().threads(num_cpus::get())
                .spawn_sym().report(&mut std::io::stdout());
        }
        Some("explore") => {
            let thread_count = args.opt_free_from_str()?
                .unwrap_or(3);
            let address = args.opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            println!("Exploring the state space of increment with {} threads on {}.",
                thread_count, address);
            State::new(thread_count)
                .checker().threads(num_cpus::get())
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
