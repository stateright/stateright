//! This module implements a subset of the two phase commit specification presented in the paper
//! ["Consensus on Transaction Commit"](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/tr-2003-96.pdf)
//! by Jim Gray and Leslie Lamport.

use clap::{App, Arg, SubCommand, value_t};
use stateright::{Model, Property};
use stateright::explorer::Explorer;
use stateright::util::{HashableHashMap, HashableHashSet};
use std::fmt::Debug;
use std::iter::FromIterator;
use std::hash::Hash;

#[derive(Clone)]
struct TwoPhaseSys<R> { pub rms: HashableHashSet<R> }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TwoPhaseState<R: Eq + Hash> {
    rm_state: HashableHashMap<R, RmState>,
    tm_state: TmState,
    tm_prepared: HashableHashSet<R>,
    msgs: HashableHashSet<Message<R>>
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Message<R> { Prepared { rm: R }, Commit, Abort }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RmState { Working, Prepared, Committed, Aborted }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TmState { Init, Committed, Aborted }

#[derive(Clone, Debug)]
enum Action<R> {
    TmRcvPrepared(R),
    TmCommit,
    TmAbort,
    RmPrepare(R),
    RmChooseToAbort(R),
    RmRcvCommitMsg(R),
    RmRcvAbortMsg(R),
}

impl<R: Clone + Eq + Hash> Model for TwoPhaseSys<R> {
    type State = TwoPhaseState<R>;
    type Action = Action<R>;

    fn init_states(&self) -> Vec<Self::State> {
        vec![TwoPhaseState {
            rm_state: self.rms.iter().map(|rm| (rm.clone(), RmState::Working)).collect(),
            tm_state: TmState::Init,
            tm_prepared: Default::default(),
            msgs: Default::default(),
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        if state.tm_state == TmState::Init && state.tm_prepared == self.rms {
            actions.push(Action::TmCommit);
        }
        if state.tm_state == TmState::Init {
            actions.push(Action::TmAbort);
        }
        for rm in &self.rms {
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
            Action::TmRcvPrepared(rm) => { state.tm_prepared.insert(rm); }
            Action::TmCommit => {
                state.tm_state = TmState::Committed;
                state.msgs.insert(Message::Commit);
            }
            Action::TmAbort => {
                state.tm_state = TmState::Aborted;
                state.msgs.insert(Message::Abort);
            },
            Action::RmPrepare(rm) => {
                state.rm_state.insert(rm.clone(), RmState::Prepared);
                state.msgs.insert(Message::Prepared { rm });
            },
            Action::RmChooseToAbort(rm) => { state.rm_state.insert(rm, RmState::Aborted); }
            Action::RmRcvCommitMsg(rm) => { state.rm_state.insert(rm, RmState::Committed); }
            Action::RmRcvAbortMsg(rm) => { state.rm_state.insert(rm, RmState::Aborted); }
        }
        Some(state)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::sometimes("abort agreement", |sys, state| {
                sys.rms.iter().all(|rm| state.rm_state[rm] == RmState::Aborted)
            }),
            Property::<Self>::sometimes("commit agreement", |sys, state| {
                sys.rms.iter().all(|rm| state.rm_state[rm] == RmState::Committed)
            }),
            Property::<Self>::always("consistent", |sys, state| {
                !sys.rms.iter().any(|rm1|
                    sys.rms.iter().any(|rm2|
                        state.rm_state[rm1] == RmState::Aborted
                            && state.rm_state[rm2] == RmState::Committed))
            }),
        ]
    }
}

#[cfg(test)]
#[test]
fn can_model_2pc() {
    // for very small state space
    let mut rms = HashableHashSet::new();
    for rm in 1..(3+1) { rms.insert(rm); }
    let mut checker = TwoPhaseSys { rms }.checker();
    assert_eq!(checker.check(300).generated_count(), 288);
    checker.assert_properties();

    // for slightly larger state space
    let mut rms = HashableHashSet::new();
    for rm in 1..(5+1) { rms.insert(rm); }
    let mut checker = TwoPhaseSys { rms }.checker();
    assert_eq!(checker.check(10_000).generated_count(), 8_832);
    checker.assert_properties();
}

fn main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("debug"));

    let mut app = App::new("2pc")
        .about("model check abstract two phase commit")
        .subcommand(SubCommand::with_name("check")
            .about("model check")
            .arg(Arg::with_name("rm_count")
                 .help("number of resource managers")
                 .default_value("7")))
        .subcommand(SubCommand::with_name("explore")
            .about("interactively explore state space")
            .arg(Arg::with_name("rm_count")
                 .help("number of resource managers")
                 .default_value("7"))
            .arg(Arg::with_name("address")
                .help("address Explorer service should listen upon")
                .default_value("localhost:3000")));
    let args = app.clone().get_matches();

    match args.subcommand() {
        ("check", Some(args)) => {
            let rm_count = value_t!(args, "rm_count", u32).expect("rm_count");
            println!("Checking two phase commit with {} resource managers.", rm_count);
            TwoPhaseSys { rms: FromIterator::from_iter(0..rm_count) }
                .checker_with_threads(num_cpus::get())
                .check_and_report(&mut std::io::stdout());
        }
        ("explore", Some(args)) => {
            let rm_count = value_t!(args, "rm_count", u32).expect("rm_count");
            let address = value_t!(args, "address", String).expect("address");
            println!("Exploring state space for two phase commit with {} resource managers on {}.", rm_count, address);
            TwoPhaseSys { rms: FromIterator::from_iter(0..rm_count) }
                .checker().serve(address).unwrap();
        }
        _ => app.print_help().unwrap(),
    }
}

