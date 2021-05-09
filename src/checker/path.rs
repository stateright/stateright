//! Private module for selective re-export.

use crate::{fingerprint, Fingerprint, Model};
use std::fmt::{Debug, Display, Formatter};
use std::collections::VecDeque;
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
    /// Constructs a path from a model and a sequence of fingerprints.
    pub(crate) fn from_fingerprints<M>(model: &M, mut fingerprints: VecDeque<Fingerprint>) -> Self
    where M: Model<State = State, Action = Action>,
          M::State: Hash,
    {
        // FIXME: Don't panic. Return a `Result` so the details can be bubbled up to Explorer/etc.
        //        Also serialize the states rather than printing the fingerprints.
        let init_print = match fingerprints.pop_front() {
            Some(init_print) => init_print,
            None => panic!("empty path is invalid"),
        };
        let mut last_state = model.init_states().into_iter()
            .find(|s| {
                fingerprint(&s) == init_print
            })
            .unwrap_or_else(|| {
                panic!(r#"
Unable to reconstruct a `Path` based on digests ("fingerprints") from states visited earlier. No
init state has the expected fingerprint ({:?}). This usually happens when the return value of
`Model::init_states` varies.

The most obvious cause would be a model that operates directly upon untracked external state such
as the file system, a thread local `RefCell`, or a source of randomness. Note that this is often
inadvertent. For example, iterating over a `HashMap` or `HashableHashMap` does not always happen in
the same order (depending on the random seed), which can lead to unexpected nondeterminism.

Available init fingerprints (none of which match): {:?}"#,
                init_print,
                model.init_states().into_iter()
                    .map(|s| fingerprint(&s))
                    .collect::<Vec<_>>());
            });
        let mut output = Vec::new();
        while let Some(next_fp) = fingerprints.pop_front() {
            let (action, next_state) = model
                .next_steps(&last_state).into_iter()
                .find_map(|(a,s)| {
                    if fingerprint(&s) == next_fp {
                        Some((a, s))
                    } else {
                        None
                    }
                }).unwrap_or_else(|| {
                    panic!(r#"
Unable to reconstruct a `Path` based on digests ("fingerprints") from states visited earlier. {}
previous state(s) of the path were able to be reconstructed, but no subsequent state has the next
fingerprint ({:?}). This usually happens when `Model::actions` or `Model::next_state` vary even
when given the same input arguments.

The most obvious cause would be a model that operates directly upon untracked external state such
as the file system, a thread local `RefCell`, or a source of randomness. Note that this is often
inadvertent. For example, iterating over a `HashMap` or `HashableHashMap` does not always happen in
the same order (depending on the random seed), which can lead to unexpected nondeterminism.

Available next fingerprints (none of which match): {:?}"#,
                    1 + output.len(),
                    next_fp,
                    model.next_states(&last_state).into_iter()
                        .map(|s| fingerprint(&s))
                        .collect::<Vec<_>>());
                });
            output.push((last_state, Some(action)));

            last_state = next_state;
        }
        output.push((last_state, None));
        Path(output)
    }

    /// Constructs a path from a model, initial state, and a sequence of actions. Panics for inputs
    /// unreachable via the model.
    pub fn from_actions<'a, M>(
        model: &M, init_state: State, actions: impl IntoIterator<Item = &'a Action>) -> Option<Self>
    where M: Model<State = State, Action = Action>,
          State: PartialEq,
          Action: PartialEq + 'a,
    {
        let mut output = Vec::new();
        if !model.init_states().contains(&init_state) { return None }
        let mut prev_state = init_state;
        for action in actions {
            let (action, next_state) = match model.next_steps(&prev_state)
                .into_iter().find(|(a, _)| a == action)
            {
                None => return None,
                Some(found) => found,
            };
            output.push((prev_state, Some(action)));
            prev_state = next_state;
        }
        output.push((prev_state, None));

        Some(Path(output))
    }

    /// Determines the final state associated with a particular fingerprint path.
    pub(crate) fn final_state<M>(model: &M, mut fingerprints: VecDeque<Fingerprint>) -> Option<M::State>
    where M: Model<State = State, Action = Action>,
          M::State: Hash,
    {
        let init_print = match fingerprints.pop_front() {
            Some(init_print) => init_print,
            None => return None,
        };
        let mut matching_state =
            match model.init_states().into_iter().find(|s| fingerprint(&s) == init_print) {
                Some(matching_state) => matching_state,
                None => return None,
            };
        while let Some(next_print) = fingerprints.pop_front() {
            matching_state =
                match model.next_states(&matching_state).into_iter().find(|s| fingerprint(&s) == next_print) {
                    Some(matching_state) => matching_state,
                    None => return None,
                };
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

    /// Encodes the path as a sequence of opaque "fingerprints" delimited by forward
    /// slash (`/`) characters.
    pub fn encode(&self) -> String where State: Hash {
        self.0.iter()
            .map(|(s, _a)| format!("{}", fingerprint(s)))
            .collect::<Vec<String>>()
            .join("/")
    }
}

impl<State, Action> From<Path<State, Action>> for Vec<(State, Option<Action>)> {
    fn from(source: Path<State, Action>) -> Self {
        source.0
    }
}

impl<State, Action> Display for Path<State, Action> 
where Action: Debug,
      State: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        writeln!(f, "Path[{}]:", self.0.len() - 1)?;
        for (_state, action) in &self.0 {
            if let Some(action) = action {
                writeln!(f, "- {:?}", action)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::iter::FromIterator;
    use std::panic::catch_unwind;
    use super::*;

    #[test]
    fn panics_if_unable_to_reconstruct_init_state() {
        let model: fn(Option<&_>, &mut Vec<_>) = |prev_state, next_states| {
            if prev_state.is_none() {
                next_states.push("UNEXPECTED");
            }
        };
        let err_result = catch_unwind(|| {
            Path::from_fingerprints(
                &model,
                VecDeque::from_iter(vec![fingerprint(&"expected")]));
        });
        assert!(err_result.is_err());
    }

    #[test]
    fn panics_if_unable_to_reconstruct_next_state() {
        let model: fn(Option<&_>, &mut Vec<_>) = |prev_state, next_states| {
            match prev_state {
                None => next_states.push("expected"),
                Some(_) => next_states.push("UNEXPECTED"),
            }
        };
        let err_result = catch_unwind(|| {
            Path::from_fingerprints(
                &model,
                VecDeque::from_iter(vec![fingerprint(&"expected"), fingerprint(&"expected")]));
        });
        assert!(err_result.is_err());
    }
}
