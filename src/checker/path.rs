//! Private module for selective re-export.

use crate::Model;
use std::collections::VecDeque;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

/// A path of states including actions. i.e. `state --action--> state ... --action--> state`.
///
/// You can convert to a `Vec<_>` with [`path.into_vec()`]. If you only need the actions, then use
/// [`path.into_actions()`].
///
/// [`path.into_vec()`]: Path::into_vec
/// [`path.into_actions()`]: Path::into_actions
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path<State, Action>(Vec<(State, Option<Action>)>);

impl<State, Action> Path<State, Action> {
    /// Constructs a path from a model and a sequence of action indices (the 0th index is for the
    /// init state).
    pub(crate) fn from_action_indices<M>(model: &M, mut indices: VecDeque<usize>) -> Self
    where
        M: Model<State = State, Action = Action>,
        State: Clone + PartialEq,
        Action: Clone + PartialEq,
    {
        // FIXME: Don't panic. Return a `Result` so the details can be bubbled up to Explorer/etc.
        //        Also serialize the states rather than printing the indices.
        let init_state_index = match indices.pop_front() {
            Some(init_index) => init_index,
            None => panic!("empty path is invalid"),
        };
        let init_states = model.init_states();
        let mut last_state = init_states
            .get(init_state_index)
            .unwrap_or_else(|| {
                panic!(
                    r#"
Unable to reconstruct a `Path` based on action indices from states visited earlier. No
init state has the expected index ({:?}). This usually happens when the return value of
`Model::init_states` varies.

The most obvious cause would be a model that operates directly upon untracked external state such
as the file system, a thread local `RefCell`, or a source of randomness. Note that this is often
inadvertent. For example, iterating over a `HashMap` or `HashableHashMap` does not always happen in
the same order (depending on the random seed), which can lead to unexpected nondeterminism.

Available init state indices (none of which match): {:?}"#,
                    init_state_index,
                    (0..init_states.len()).collect::<Vec<_>>()
                );
            })
            .clone();
        let mut output = Vec::new();
        while let Some(action_index) = indices.pop_front() {
            let mut actions = Vec::new();
            model.actions(&last_state, &mut actions);
            let action = actions
                .get(action_index)
                .unwrap_or_else(|| {
                    panic!(
                        r#"
Unable to reconstruct a `Path` based on action indices from states visited earlier. {}
previous state(s) of the path were able to be reconstructed, but no action has the next
index ({:?}). This usually happens when `Model::actions` or `Model::next_state` vary even
when given the same input arguments.

The most obvious cause would be a model that operates directly upon untracked external state such
as the file system, a thread local `RefCell`, or a source of randomness. Note that this is often
inadvertent. For example, iterating over a `HashMap` or `HashableHashMap` does not always happen in
the same order (depending on the random seed), which can lead to unexpected nondeterminism.

Available action indices (none of which match): {:?}"#,
                        1 + output.len(),
                        action_index,
                        (0..actions.len()).collect::<Vec<_>>()
                    );
                })
                .clone();
            output.push((last_state.clone(), Some(action.clone())));

            if let Some(next_state) = model.next_state(&last_state, action.clone()) {
                last_state = next_state;
            }
        }
        output.push((last_state, None));
        Path(output)
    }

    /// Constructs a path from a model, initial state, and a sequence of actions. Panics for inputs
    /// unreachable via the model.
    pub fn from_actions<'a, M>(
        model: &M,
        init_state: State,
        actions: impl IntoIterator<Item = &'a Action>,
    ) -> Option<Self>
    where
        M: Model<State = State, Action = Action>,
        State: PartialEq,
        Action: PartialEq + 'a,
    {
        let mut output = Vec::new();
        if !model.init_states().contains(&init_state) {
            return None;
        }
        let mut prev_state = init_state;
        for action in actions {
            let (action, next_state) = model
                .next_steps(&prev_state)
                .into_iter()
                .find(|(a, _)| a == action)?;
            output.push((prev_state, Some(action)));
            prev_state = next_state;
        }
        output.push((prev_state, None));

        Some(Path(output))
    }

    /// Determines the final state associated with a particular action indices path.
    pub(crate) fn final_state<M>(model: &M, mut indices: VecDeque<usize>) -> Option<M::State>
    where
        M: Model<State = State, Action = Action>,
        State: Clone + PartialEq,
        Action: Clone + PartialEq,
    {
        let init_state_index = indices.pop_front()?;
        let mut matching_state = model.init_states().get(init_state_index)?.clone();
        while let Some(action_index) = indices.pop_front() {
            let mut actions = Vec::new();
            model.actions(&matching_state, &mut actions);
            let action = actions.get(action_index)?.clone();
            if let Some(next_state) = model.next_state(&matching_state, action.clone()) {
                matching_state = next_state;
            }
        }
        Some(matching_state)
    }

    /// Extracts the last state.
    pub fn last_state(&self) -> &State {
        &self.0.last().unwrap().0
    }

    /// Extracts the states.
    pub fn into_states(self) -> Vec<State> {
        self.0.into_iter().map(|(s, _a)| s).collect()
    }

    /// Extracts the actions.
    pub fn into_actions(self) -> Vec<Action> {
        self.0.into_iter().filter_map(|(_s, a)| a).collect()
    }

    /// Convenience method for `Into<Vec<_>>`.
    pub fn into_vec(self) -> Vec<(State, Option<Action>)> {
        self.into()
    }

    /// Encodes the path as a sequence of action indices (the 0th index is for the init state)
    /// delimited by forward lash (`/`) characters.
    pub fn encode<M>(&self, model: &M) -> String
    where
        M: Model<State = State, Action = Action>,
        State: PartialEq,
        Action: PartialEq,
    {
        let mut result = Vec::new();
        let init_state = &self.0[0].0;
        let init_index = model
            .init_states()
            .iter()
            .position(|s| s == init_state)
            .expect("Init state not found in model's init_states");
        result.push(init_index.to_string());
        for i in 0..self.0.len() - 1 {
            let (state, action) = &self.0[i];
            if let Some(action) = action {
                let mut actions = Vec::new();
                model.actions(state, &mut actions);
                let action_index = actions
                    .iter()
                    .position(|a| a == action)
                    .expect("Action not found in model's actions");
                result.push(action_index.to_string());
            }
        }
        result.join("/")
    }
}

impl<State, Action> From<Path<State, Action>> for Vec<(State, Option<Action>)> {
    fn from(source: Path<State, Action>) -> Self {
        source.0
    }
}

impl<State, Action> Display for Path<State, Action>
where
    Action: Debug,
    State: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        writeln!(f, "Path[{}]:", self.0.len() - 1)?;
        for (_state, action) in &self.0 {
            if let Some(action) = action {
                writeln!(f, "- {action:?}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::iter::FromIterator;
    use std::panic::catch_unwind;

    #[test]
    fn panics_if_unable_to_reconstruct_init_state() {
        let model: fn(Option<&_>, &mut Vec<_>) = |prev_state, next_states| {
            if prev_state.is_none() {
                next_states.push("UNEXPECTED");
            }
        };
        let err_result = catch_unwind(|| {
            Path::from_action_indices(&model, VecDeque::from_iter(vec![999]));
        });
        assert!(err_result.is_err());
    }

    #[test]
    fn panics_if_unable_to_reconstruct_next_state() {
        let model: fn(Option<&_>, &mut Vec<_>) = |prev_state, next_states| match prev_state {
            None => next_states.push("expected"),
            Some(_) => next_states.push("UNEXPECTED"),
        };
        let err_result = catch_unwind(|| {
            Path::from_action_indices(&model, VecDeque::from_iter(vec![0, 999]));
        });
        assert!(err_result.is_err());
    }
}
