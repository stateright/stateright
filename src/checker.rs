//! Private module for selective re-export.

mod bfs;
use crate::{fingerprint, Fingerprint, Expectation, Model};
mod dfs;
mod explorer;
mod visitor;
use std::collections::{HashMap, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::time::Instant;

pub use visitor::*;

/// A [`Model`] [`Checker`] builder. Instantiable via the [`Model::checker`] method.
///
/// # Example
///
/// ```
/// # use stateright::*; let model = ();
/// model.checker().threads(4).spawn_dfs().join().assert_properties();
/// ```
#[must_use = "This code constructs a builder, not a checker. \
              Consider calling spawn_bfs() or spawn_dfs()."]
pub struct CheckerBuilder<M: Model> {
    model: M,
    target_generated_count: Option<NonZeroUsize>,
    thread_count: usize,
    visitor: Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
}
impl<M: Model> CheckerBuilder<M> {
    pub(crate) fn new(model: M) -> Self {
        Self {
            model,
            target_generated_count: None,
            thread_count: 1,
            visitor: None,
        }
    }

    /// Spawns a breadth-first search model checker. This call does not block the current thread. You
    /// can call [`Checker::join`] to block a thread until checking completes.
    #[must_use = "Checkers run on background threads. \
                  Consider calling join(), report(...), or serve(...), for example."]
    pub fn spawn_bfs(self) -> impl Checker<M>
    where M: Model + Send + Sync + 'static,
          M::State: Hash + Send + Sync + 'static,
    {
        bfs::BfsChecker::spawn(self)
    }

    /// Spawns a depth-first search model checker. This call does not block the current thread. You
    /// can call [`Checker::join`] to block a thread until checking completes.
    #[must_use = "Checkers run on background threads. \
                  Consider calling join(), report(...), or serve(...), for example."]
    pub fn spawn_dfs(self) -> impl Checker<M>
    where M: Model + Send + Sync + 'static,
          M::State: Hash + Send + Sync + 'static,
    {
        dfs::DfsChecker::spawn(self)
    }

    /// Sets the number of states that the checker should aim to generate. For performance reasons
    /// the checker may exceed this number, but it will never generate fewer states if more exist.
    pub fn target_generated_count(self, target_generated_count: usize) -> Self {
        Self { target_generated_count: NonZeroUsize::new(target_generated_count), .. self }
    }

    /// Sets the number of threads available for model checking. For maximum performance this
    /// should match the number of cores.
    pub fn threads(self, thread_count: usize) -> Self {
        Self { thread_count, .. self }
    }

    /// Indicates a function to be run on each evaluated state.
    pub fn visitor(self, visitor: impl CheckerVisitor<M> + Send + Sync + 'static) -> Self {
        Self { visitor: Some(Box::new(visitor)), .. self }
    }
}

/// A path of states including actions. i.e. `state --action--> state ... --action--> state`.
///
/// You can convert to a `Vec<_>` with [`path.into_vec()`]. If you only need the actions, then use
/// [`path.into_actions()`].
///
/// [`path.into_vec()`]: Path::into_vec
/// [`path.into_actions()`]: Path::into_actions
#[derive(Clone, Debug, PartialEq)]
pub struct Path<State, Action>(Vec<(State, Option<Action>)>);
impl<State, Action> Path<State, Action> {
    /// Constructs a path from a model and a sequence of fingerprints.
    fn from_fingerprints<M>(model: &M, mut fingerprints: VecDeque<Fingerprint>) -> Self
    where M: Model<State = State, Action = Action>,
          M::State: Hash,
    {
        let init_print = match fingerprints.pop_front() {
            Some(init_print) => init_print,
            None => panic!("empty path is invalid"),
        };
        let mut last_state = model.init_states().into_iter()
            .find(|s| {
                fingerprint(&s) == init_print
            })
            .expect("no init state matches fingerprint");
        let mut output = Vec::new();
        while let Some(next_fp) = fingerprints.pop_front() {
            let mut actions = Vec::new();
            model.actions(
                &last_state,
                &mut actions);

            let (action, next_state) = model
                .next_steps(&last_state).into_iter()
                .find_map(|(a,s)| {
                    if fingerprint(&s) == next_fp {
                        Some((a, s))
                    } else {
                        None
                    }
                }).expect("no next state matches fingerprint");
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
    fn final_state<M>(model: &M, mut fingerprints: VecDeque<Fingerprint>) -> Option<M::State>
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
impl<State, Action> Into<Vec<(State, Option<Action>)>> for Path<State, Action> {
    fn into(self) -> Vec<(State, Option<Action>)> { self.0 }
}

/// Implementations perform [`Model`] checking.
///
/// Call [`Model::checker`] to instantiate a [`CheckerBuilder`]. Then call
/// [`CheckerBuilder::spawn_dfs`] or [`CheckerBuilder::spawn_bfs`].
pub trait Checker<M: Model> {
    /// Returns a reference to this checker's [`Model`].
    fn model(&self) -> &M;

    /// Indicates how many states have been generated.
    fn generated_count(&self) -> usize;

    /// Returns a map from property name to corresponding "discovery" (indicated
    /// by a [`Path`]).
    fn discoveries(&self) -> HashMap<&'static str, Path<M::State, M::Action>>;

    /// Blocks the current thread until checking [`is_done`] or each thread evaluates
    /// a specified maximum number of states.
    ///
    /// [`is_done`]: Self::is_done
    fn join(self) -> Self;

    /// Indicates that either all properties have associated discoveries or all reachable states
    /// have been visited.
    fn is_done(&self) -> bool;

    /// Starts a web service for interactively exploring a model.
    ///
    /// ![Stateright Explorer screenshot](https://raw.githubusercontent.com/stateright/stateright/master/explorer.jpg)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stateright::{Checker, Model};
    ///
    /// #[derive(Clone, Debug, Hash)]
    /// enum FizzBuzzAction { Fizz, Buzz, FizzBuzz }
    /// #[derive(Clone)]
    /// struct FizzBuzzModel { max: usize }
    ///
    /// impl Model for FizzBuzzModel {
    ///     type State = Vec<(usize, Option<FizzBuzzAction>)>;
    ///     type Action = Option<FizzBuzzAction>;
    ///     fn init_states(&self) -> Vec<Self::State> {
    ///         vec![Vec::new()]
    ///     }
    ///     fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
    ///         actions.push(
    ///             if state.len() % 15 == 0 {
    ///                 Some(FizzBuzzAction::FizzBuzz)
    ///             } else if state.len() % 5 == 0 {
    ///                 Some(FizzBuzzAction::Buzz)
    ///             } else if state.len() % 3 == 0 {
    ///                 Some(FizzBuzzAction::Fizz)
    ///             } else {
    ///                 None
    ///             });
    ///     }
    ///     fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
    ///         let mut state = state.clone();
    ///         state.push((state.len(), action));
    ///         Some(state)
    ///     }
    ///     fn within_boundary(&self, state: &Self::State) -> bool {
    ///         state.len() <= self.max
    ///     }
    /// }
    ///
    /// let _ = FizzBuzzModel { max: 30 }.checker().spawn_dfs().serve("localhost:3000");
    /// ```
    ///
    /// # API
    ///
    /// - `GET /` returns a web browser UI as HTML.
    /// - `GET /.status` returns information about the model checker status.
    /// - `GET /.states` returns available initial states and fingerprints.
    /// - `GET /.states/{fingerprint1}/{fingerprint2}/...` follows the specified
    ///    path of fingerprints and returns available actions with resulting
    ///    states and fingerprints.
    /// - `GET /.states/.../{invalid-fingerprint}` returns 404.
    fn serve(self, addresses: impl std::net::ToSocketAddrs) -> std::sync::Arc<Self>
    where M: 'static + Model,
          M::Action: Debug,
          M::State: Debug + Hash,
          Self: 'static + Send + Sized + Sync,
    {
        explorer::serve(self, addresses)
    }

    /// Looks up a discovery by property name. Panics if the property does not exist.
    fn discovery(&self, name: &'static str) -> Option<Path<M::State, M::Action>> {
        self.discoveries().remove(name)
    }

    /// Periodically emits a status message.
    fn report(self, w: &mut impl std::io::Write) -> Self where Self: Sized {
        let method_start = Instant::now();
        while !self.is_done() {
            let _ = writeln!(w, "Checking. generated={}", self.generated_count());
            std::thread::sleep(std::time::Duration::from_millis(1_000));
        }
        let _ = writeln!(w, "Done. generated={}, sec={}",
                 self.generated_count(),
                 method_start.elapsed().as_secs());
        self
    }

    /// A helper that verifies examples exist for all `sometimes` properties and no counterexamples
    /// exist for any `always`/`eventually` properties.
    fn assert_properties(&self)
    where M::Action: Debug,
          M::State: Debug,
    {
        for p in self.model().properties() {
            match p.expectation {
                Expectation::Always => self.assert_no_discovery(p.name),
                Expectation::Eventually => self.assert_no_discovery(p.name),
                Expectation::Sometimes => { self.assert_any_discovery(p.name); },
            }
        }
    }

    /// Panics if a particular discovery is not found.
    fn assert_any_discovery(&self, name: &'static str) -> Path<M::State, M::Action> {
        if let Some(found) = self.discovery(name) { return found }
        assert!(self.is_done(),
                "Discovery for '{}' not found, but model checking is incomplete.", name);
        panic!("Discovery for '{}' not found.", name);
    }

    /// Panics if a particular discovery is found.
    fn assert_no_discovery(&self, name: &'static str)
    where M::Action: Debug,
          M::State: Debug,
    {
        if let Some(found) = self.discovery(name) {
            let last_state = format!("{:#?}", found.last_state());
            let actions = found.into_actions()
                .iter()
                .map(|a| format!("{:?}", a))
                .collect::<Vec<_>>()
                .join("\n");
            panic!("Discovery for '{}' found.\n\n== ACTIONS ==\n{}\n\n== LAST STATE ==\n{}",
                   name, actions, last_state);
        }
        assert!(self.is_done(),
                "Discovery for '{}' not found, but model checking is incomplete.",
                name);
    }

    /// Panics if the specified actions do not result in a discovery for the specified property
    /// name.
    fn assert_discovery(&self, name: &'static str, actions: Vec<M::Action>)
    where M::State: Debug + PartialEq,
          M::Action: Debug + PartialEq,
    {
        let found = self.assert_any_discovery(name);
        for init_state in self.model().init_states() {
            if let Some(path) = Path::from_actions(self.model(), init_state, &actions) {
                let property = self.model().property(name);
                match property.expectation {
                    Expectation::Always => {
                        if !(property.condition)(self.model(), path.last_state()) { return }
                    }
                    Expectation::Eventually => {
                        if actions != self.assert_any_discovery(name).into_actions() {
                            todo!("Not yet able to validate eventually properties \
                                   unless they are discovered by the model checker,
                                   as this implementation does not check that the
                                   path is terminal.");
                        }
                        if !(property.condition)(self.model(), path.last_state()) { return }
                    }
                    Expectation::Sometimes => {
                        if (property.condition)(self.model(), path.last_state()) { return }
                    }
                }
            }
        }
        panic!("Invalid discovery for '{}', but a valid one was found. found={:?}",
               name, found.into_actions());
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::linear_equation_solver::LinearEquation;

    #[test]
    fn can_build_path_from_fingerprints() {
        let fp = |a: u8, b: u8| fingerprint(&(a, b));
        let model = LinearEquation { a: 2, b: 10, c: 14 };
        let fingerprints = VecDeque::from(vec![
            fp(0, 0),
            fp(0, 1),
            fp(1, 1),
            fp(2, 1), // final state
        ]);
        let path = Path::from_fingerprints(&model, fingerprints.clone());
        assert_eq!(
            path.last_state(),
            &(2,1));
        assert_eq!(
            path.last_state(),
            &Path::final_state(&model, fingerprints).unwrap());
    }

    #[test]
    fn report_includes_property_names_and_paths() {
        // The assertions use `starts_with` to omit timing since it varies.

        // BFS
        let mut written: Vec<u8> = Vec::new();
        LinearEquation { a: 2, b: 10, c: 14 }.checker()
            .spawn_bfs().report(&mut written);
        let output = String::from_utf8(written).unwrap();
        assert!(
            output.starts_with("\
                Checking. generated=1\n\
                Done. generated=12, sec="),
            "Output did not start as expected (see test). output={:?}`", output);

        // DFS
        let mut written: Vec<u8> = Vec::new();
        LinearEquation { a: 2, b: 10, c: 14 }.checker()
            .spawn_dfs().report(&mut written);
        let output = String::from_utf8(written).unwrap();
        assert!(
            output.starts_with("\
                Checking. generated=1\n\
                Done. generated=55, sec="),
            "Output did not start as expected (see test). output={:?}`", output);
    }
}
