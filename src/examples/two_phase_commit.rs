//! This module implements a subset of the two phase commit specification presented in the paper
//! ["Consensus on Transaction Commit"](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/tr-2003-96.pdf)
//! by Jim Gray and Leslie Lamport.

use ::*;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::hash::Hash;

pub struct TwoPhaseModel<R> { pub rms: BTreeSet<R> }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TwoPhaseState<R> {
    rm_state: BTreeMap<R, RmState>,
    tm_state: TmState,
    tm_prepared: BTreeSet<R>,
    msgs: BTreeSet<Message<R>>
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, PartialEq)]
pub enum Message<R> { Prepared { rm: R }, Commit, Abort }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RmState { Working, Prepared, Committed, Aborted }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TmState { Init, Committed, Aborted }

impl<R: Clone + Eq + Hash + Ord> TwoPhaseModel<R> {
    fn tm_rcv_prepared(&self, rm: &R, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.tm_state == TmState::Init
                && state.msgs.contains(&Message::Prepared { rm: rm.clone() }) {
            let mut result = state.clone();
            result.tm_prepared.insert(rm.clone());
            results.push_back(("TM got prepared msg", result));
        }
    }
    fn tm_commit(&self, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.tm_state == TmState::Init
                && state.tm_prepared == self.rms {
            let mut result = state.clone();
            result.tm_state = TmState::Committed;
            result.msgs.insert(Message::Commit);
            results.push_back(("TM was able to commit and has informed RMs", result));
        }
    }
    fn tm_abort(&self, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.tm_state == TmState::Init {
            let mut result = state.clone();
            result.tm_state = TmState::Aborted;
            result.msgs.insert(Message::Abort);
            results.push_back(("TM chose to abort", result));
        }
    }
    fn rm_prepare(&self, rm: &R, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.rm_state.get(rm) == Some(&RmState::Working) {
            let mut result = state.clone();
            result.rm_state.insert(rm.clone(), RmState::Prepared);
            result.msgs.insert(Message::Prepared { rm: rm.clone() });
            results.push_back(("RM is preparing", result));
        }
    }
    fn rm_choose_to_abort(&self, rm: &R, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.rm_state.get(rm) == Some(&RmState::Working) {
            let mut result = state.clone();
            result.rm_state.insert(rm.clone(), RmState::Aborted);
            results.push_back(("RM is choosing to abort", result));
        }
    }
    fn rm_rcv_commit_msg(&self, rm: &R, state: &TwoPhaseState<R>, results: &mut VecDeque<Step<TwoPhaseState<R>>>) {
        if state.msgs.contains(&Message::Commit) {
            let mut result = state.clone();
            result.rm_state.insert(rm.clone(), RmState::Committed);
            results.push_back(("RM is being told to commit", result));
        }
    }
    fn rm_rcv_abort_msg(&self, rm: &R, state: &TwoPhaseState<R>, results: &mut VecDeque<(&'static str, TwoPhaseState<R>)>) {
        if state.msgs.contains(&Message::Abort) {
            let mut result = state.clone();
            result.rm_state.insert(rm.clone(), RmState::Aborted);
            results.push_back(("RM is being told to abort", result));
        }
    }
}

impl<R: Clone + Eq + Hash + Ord> StateMachine for TwoPhaseModel<R> {
    type State = TwoPhaseState<R>;
    fn init(&self, results: &mut VecDeque<Step<Self::State>>) {
        let state = TwoPhaseState {
            rm_state: self.rms.iter().map(|rm| (rm.clone(), RmState::Working)).collect(),
            tm_state: TmState::Init,
            tm_prepared: BTreeSet::new(),
            msgs: BTreeSet::new()
        };
        results.push_back(("init", state));
    }

    fn next(&self, state: &Self::State, results: &mut VecDeque<Step<Self::State>>) {
        self.tm_commit(state, results);
        self.tm_abort(state, results);
        for rm in &self.rms {
            self.tm_rcv_prepared(rm, state, results);
            self.rm_prepare(rm, state, results);
            self.rm_choose_to_abort(rm, state, results);
            self.rm_rcv_commit_msg(rm, state, results);
            self.rm_rcv_abort_msg(rm, state, results);
        }
    }
}

impl<R: Clone + Eq + Hash + Ord> Model for TwoPhaseModel<R> {
    fn invariant(&self, state: &Self::State) -> bool {
        !self.rms.iter().any(|rm1|
            self.rms.iter().any(|rm2|
                state.rm_state[rm1] == RmState::Aborted && state.rm_state[rm2] == RmState::Committed))
    }
}

#[cfg(test)]
mod test {
    use ::*;
    use ::examples::two_phase_commit::*;

    #[test]
    fn can_model_2pc() {
        let mut rms = BTreeSet::new();
        for rm in 1..(5+1) {
            rms.insert(rm);
        }
        let model = TwoPhaseModel { rms };
        let mut checker = model.checker(false);
        assert_eq!(
            checker.check(1_000_000),
            CheckResult::Pass);
        assert_eq!(
            checker.visited.len(),
            8832);
    }
}

