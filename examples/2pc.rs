//! This module implements a subset of the two phase commit specification presented in the paper
//! ["Consensus on Transaction Commit"](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/tr-2003-96.pdf)
//! by Jim Gray and Leslie Lamport.

use stateright::{Checker, Model, Property, Reindex, Strategy, Symmetric};
use std::collections::BTreeSet;
use std::hash::Hash;
use std::ops::Range;

type R = usize; // represented by integers in 0..N-1

#[derive(Clone)]
struct TwoPhaseSys { pub rms: Range<R> }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TwoPhaseState {
    rm_state: Vec<RmState>, // map from each RM
    tm_state: TmState,
    tm_prepared: Vec<bool>, // map from each RM
    msgs: BTreeSet<Message>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Message { Prepared { rm: R }, Commit, Abort }

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum RmState { Working, Prepared, Committed, Aborted }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TmState { Init, Committed, Aborted }

#[derive(Clone, Debug)]
enum Action {
    TmRcvPrepared(R),
    TmCommit,
    TmAbort,
    RmPrepare(R),
    RmChooseToAbort(R),
    RmRcvCommitMsg(R),
    RmRcvAbortMsg(R),
}

impl Model for TwoPhaseSys {
    type State = TwoPhaseState;
    type Action = Action;

    fn init_states(&self) -> Vec<Self::State> {
        vec![TwoPhaseState {
            rm_state: self.rms.clone().map(|_| RmState::Working).collect(),
            tm_state: TmState::Init,
            tm_prepared: self.rms.clone().map(|_| false).collect(),
            msgs: Default::default(),
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        if state.tm_state == TmState::Init && state.tm_prepared.iter().all(|p| *p) {
            actions.push(Action::TmCommit);
        }
        if state.tm_state == TmState::Init {
            actions.push(Action::TmAbort);
        }
        for rm in self.rms.clone() {
            if state.tm_state == TmState::Init
                    && state.msgs.contains(&Message::Prepared { rm: rm.clone() }) {
                actions.push(Action::TmRcvPrepared(rm.clone()));
            }
            if state.rm_state.get(rm) == Some(&RmState::Working) {
                actions.push(Action::RmPrepare(rm.clone()));
            }
            if state.rm_state.get(rm) == Some(&RmState::Working) {
                actions.push(Action::RmChooseToAbort(rm.clone()));
            }
            if state.msgs.contains(&Message::Commit) {
                actions.push(Action::RmRcvCommitMsg(rm.clone()));
            }
            if state.msgs.contains(&Message::Abort) {
                actions.push(Action::RmRcvAbortMsg(rm.clone()));
            }
        }
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut state = last_state.clone();
        match action.clone() {
            Action::TmRcvPrepared(rm) => { state.tm_prepared[rm] = true; }
            Action::TmCommit => {
                state.tm_state = TmState::Committed;
                state.msgs.insert(Message::Commit);
            }
            Action::TmAbort => {
                state.tm_state = TmState::Aborted;
                state.msgs.insert(Message::Abort);
            },
            Action::RmPrepare(rm) => {
                state.rm_state[rm] = RmState::Prepared;
                state.msgs.insert(Message::Prepared { rm });
            },
            Action::RmChooseToAbort(rm) => { state.rm_state[rm] = RmState::Aborted; }
            Action::RmRcvCommitMsg(rm) => { state.rm_state[rm] = RmState::Committed; }
            Action::RmRcvAbortMsg(rm) => { state.rm_state[rm] = RmState::Aborted; }
        }
        Some(state)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::sometimes("abort agreement", |_, state| {
                state.rm_state.iter().all(|s| s == &RmState::Aborted)
            }),
            Property::<Self>::sometimes("commit agreement", |_, state| {
                state.rm_state.iter().all(|s| s == &RmState::Committed)
            }),
            Property::<Self>::always("consistent", |_, state| {
               !state.rm_state.iter().any(|s1|
                    state.rm_state.iter().any(|s2|
                        s1 == &RmState::Aborted && s2 == &RmState::Committed))
            }),
        ]
    }
}

// Implementing this trait enables symmetry reduction to speed up model checking (optional).
impl Symmetric for TwoPhaseState {
    fn permutations(&self) -> Box<dyn Iterator<Item = Self>> {
        unimplemented!()
    }

    fn a_sorted_permutation(&self) -> Self {
        // This implementation canonicalizes the resource manager states by sorting them. It then
        // rewrites resource manager identifiers accordingly.
        let mut combined = self.rm_state.iter()
            .enumerate()
            .map(|(i, s)| (s, i))
            .collect::<Vec<_>>();
        combined.sort_unstable();

        let mapping: Vec<usize> = combined.iter()
            .map(|(_, i)| combined[*i].1)
            .collect();
        Self {
            rm_state: combined.into_iter()
                .map(|(s, _)| s.clone())
                .collect(),
            tm_state: self.tm_state.clone(),
            tm_prepared: self.tm_prepared.reindex(&mapping),
            msgs: self.msgs.iter().map(|m| {
                match m {
                    Message::Prepared { rm } => Message::Prepared { rm: mapping[*rm] },
                    Message::Commit => Message::Commit,
                    Message::Abort => Message::Abort,
                }
            }).collect(),
        }
    }
}

#[cfg(test)]
#[test]
fn can_model_2pc() {
    // for very small state space (using BFS this time)
    let checker = TwoPhaseSys { rms: 0..3 }.checker().spawn_bfs().join();
    assert_eq!(checker.unique_state_count(), 288);
    checker.assert_properties();

    // for slightly larger state space (using DFS this time)
    let checker = TwoPhaseSys { rms: 0..5 }.checker().spawn_dfs().join();
    assert_eq!(checker.unique_state_count(), 8_832);
    checker.assert_properties();

    // reverify the larger state space with symmetry reduction
    let checker = TwoPhaseSys { rms: 0..5 }.checker().spawn_sym(Strategy::Sorted).join();
    assert_eq!(checker.unique_state_count(), 3_131);
    checker.assert_properties();
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let rm_count = args.opt_free_from_str()?
                .unwrap_or(2);
            println!("Checking two phase commit with {} resource managers.", rm_count);
            TwoPhaseSys { rms: 0..rm_count }.checker()
                .threads(num_cpus::get()).spawn_dfs()
                .report(&mut std::io::stdout());
        }
        Some("check-sym") => {
            let rm_count = args.opt_free_from_str()?
                .unwrap_or(2);
            println!("Checking two phase commit with {} resource managers using symmetry reduction.", rm_count);
            TwoPhaseSys { rms: 0..rm_count }.checker()
                .threads(num_cpus::get()).spawn_sym(Strategy::Sorted)
                .report(&mut std::io::stdout());
        }
        Some("explore") => {
            let rm_count = args.opt_free_from_str()?
                .unwrap_or(2);
            let address = args.opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            println!("Exploring state space for two phase commit with {} resource managers on {}.", rm_count, address);
            TwoPhaseSys { rms: 0..rm_count }.checker()
                .threads(num_cpus::get())
                .serve(address);
        }
        _ => {
            println!("USAGE:");
            println!("  ./2pc check [RESOURCE_MANAGER_COUNT]");
            println!("  ./2pc check-sym [RESOURCE_MANAGER_COUNT]");
            println!("  ./2pc explore [RESOURCE_MANAGER_COUNT] [ADDRESS]");
        }
    }

    Ok(())
}

