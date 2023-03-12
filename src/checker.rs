//! Private module for selective re-export.

mod bfs;
mod dfs;
mod explorer;
mod on_demand;
mod path;
mod representative;
mod rewrite;
mod rewrite_plan;
mod visitor;

use crate::report::{ReportData, ReportDiscovery, Reporter};
use crate::{Expectation, Fingerprint, Model};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::time::Instant;

pub use path::*;
pub use representative::*;
pub use rewrite::*;
pub use rewrite_plan::*;
pub use visitor::*;

#[derive(Clone, Copy)]
pub(crate) enum ControlFlow {
    CheckFingerprint(Fingerprint),
    RunToCompletion,
}

/// The classification of a property discovery.
pub enum DiscoveryClassification {
    /// An example has been found.
    Example,
    /// A counterexample has been found.
    Counterexample,
}

impl Display for DiscoveryClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryClassification::Example => write!(f, "example"),
            DiscoveryClassification::Counterexample => write!(f, "counterexample"),
        }
    }
}

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
    #[allow(clippy::type_complexity)]
    symmetry: Option<fn(&M::State) -> M::State>,
    target_state_count: Option<NonZeroUsize>,
    target_max_depth: Option<NonZeroUsize>,
    thread_count: usize,
    visitor: Option<Box<dyn CheckerVisitor<M> + Send + Sync>>,
}
impl<M: Model> CheckerBuilder<M> {
    pub(crate) fn new(model: M) -> Self {
        Self {
            model,
            target_state_count: None,
            target_max_depth: None,
            symmetry: None,
            thread_count: 1,
            visitor: None,
        }
    }

    /// Starts a web service for interactively exploring a model ([demo](http://demo.stateright.rs:3000/)).
    ///
    /// ![Stateright Explorer screenshot](https://raw.githubusercontent.com/stateright/stateright/master/explorer.png)
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
    /// let _ = FizzBuzzModel { max: 30 }.checker().serve("localhost:3000");
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
    pub fn serve(self, addresses: impl std::net::ToSocketAddrs) -> std::sync::Arc<impl Checker<M>>
    where
        M: 'static + Model + Send + Sync,
        M::Action: Debug + Send + Sync,
        M::State: Debug + Hash + Send + Sync,
    {
        explorer::serve(self, addresses)
    }

    /// Spawns a breadth-first search model checker. This traversal strategy uses more memory than
    /// [`CheckerBuilder::spawn_dfs`] but will find the shortest [`Path`] to each discovery if
    /// checking is single threadeded (the default behavior, which [`CheckerBuilder::threads`]
    /// overrides).
    ///
    /// This call does not block the current thread. Call [`Checker::join`] to block until checking
    /// completes.
    #[must_use = "Checkers run on background threads. \
                  Consider calling join() or report(...), for example."]
    pub fn spawn_bfs(self) -> impl Checker<M>
    where
        M: Model + Send + Sync + 'static,
        M::State: Hash + Send + Sync + 'static,
    {
        bfs::BfsChecker::spawn(self)
    }

    /// Spawns an on-demand model checker. This traversal strategy doesn't compute any states until
    /// it is asked to, useful for lightweight exploration. Internally the exploration strategy is
    /// very similar to that of [`CheckerBuilder::spawn_bfs`].
    ///
    /// This call does not block the current thread. Call [`Checker::join`] to block until checking
    /// completes.
    #[must_use = "Checkers run on background threads. \
                  Consider calling join() or report(...), for example."]
    pub fn spawn_on_demand(self) -> impl Checker<M>
    where
        M: Model + Send + Sync + 'static,
        M::State: Hash + Send + Sync + 'static,
    {
        on_demand::OnDemandChecker::spawn(self)
    }

    /// Spawns a depth-first search model checker. This traversal strategy uses dramatically less
    /// memory than [`CheckerBuilder::spawn_bfs`] at the cost of not finding the shortest [`Path`]
    /// to each discovery.
    ///
    /// This call does not block the current thread. Call [`Checker::join`] to block until
    /// checking completes.
    #[must_use = "Checkers run on background threads. \
                  Consider calling join() or report(...), for example."]
    pub fn spawn_dfs(self) -> impl Checker<M>
    where
        M: Model + Send + Sync + 'static,
        M::State: Hash + Send + Sync + 'static,
    {
        dfs::DfsChecker::spawn(self)
    }

    /// Enables symmetry reduction. Requires the [model state] to implement [`Representative`].
    ///
    /// [model state]: crate::Model::State
    pub fn symmetry(self) -> Self
    where
        M::State: Representative,
    {
        self.symmetry_fn(Representative::representative)
    }

    /// Enables symmetry reduction based on a representative function.
    ///
    /// [model state]: crate::Model::State
    pub fn symmetry_fn(self, representative: fn(&M::State) -> M::State) -> Self {
        Self {
            symmetry: Some(representative),
            ..self
        }
    }

    /// Sets the number of states that the checker should aim to generate. For performance reasons
    /// the checker may exceed this number, but it will never generate fewer states if more exist.
    pub fn target_state_count(self, count: usize) -> Self {
        Self {
            target_state_count: NonZeroUsize::new(count),
            ..self
        }
    }

    /// Sets the maximum depth that the checker should aim to explore.
    pub fn target_max_depth(self, depth: usize) -> Self {
        Self {
            target_max_depth: NonZeroUsize::new(depth),
            ..self
        }
    }

    /// Sets the number of threads available for model checking. For maximum performance this
    /// should match the number of cores.
    pub fn threads(self, thread_count: usize) -> Self {
        Self {
            thread_count,
            ..self
        }
    }

    /// Indicates a function to be run on each evaluated state.
    pub fn visitor(self, visitor: impl CheckerVisitor<M> + Send + Sync + 'static) -> Self {
        Self {
            visitor: Some(Box::new(visitor)),
            ..self
        }
    }
}

/// Implementations perform [`Model`] checking.
///
/// Call [`Model::checker`] to instantiate a [`CheckerBuilder`]. Then call
/// [`CheckerBuilder::spawn_dfs`] or [`CheckerBuilder::spawn_bfs`].
pub trait Checker<M: Model> {
    /// Returns a reference to this checker's [`Model`].
    fn model(&self) -> &M;

    /// Asks the checker to check the given fingerprint if not done already.
    fn check_fingerprint(&self, _fingerprint: Fingerprint) {
        // nothing to do for most cases
    }

    /// Asks the checker to run to completion, no longer waiting for fingerprints.
    fn run_to_completion(&self) {
        // nothing to do for most cases
    }

    /// Indicate how many states have been generated including repeats. Always greater than or
    /// equal to [`Checker::unique_state_count`].
    fn state_count(&self) -> usize;

    /// Indicates how many unique states have been generated. Always less than or equal to
    /// [`Checker::state_count`].
    fn unique_state_count(&self) -> usize;

    /// Indicates the maximum depth that has been explored.
    fn max_depth(&self) -> usize;

    /// List of out degree of each state.
    /// The out degree is the number of actions generated by the state.
    fn out_degrees(&self) -> Vec<usize>;

    /// List of in degree of each state.
    /// The in degree is the number of actions that lead into the state.
    fn in_degrees(&self) -> Vec<usize>;

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

    /// Looks up a discovery by property name. Panics if the property does not exist.
    fn discovery(&self, name: &'static str) -> Option<Path<M::State, M::Action>> {
        self.discoveries().remove(name)
    }

    /// Periodically emits a status message.
    fn report<R>(self, reporter: &mut R) -> Self
    where
        M::Action: Debug,
        M::State: Debug,
        Self: Sized,
        R: Reporter<M>,
    {
        // Start with the checking status.
        let method_start = Instant::now();
        while !self.is_done() {
            reporter.report_checking(ReportData {
                total_states: self.state_count(),
                unique_states: self.unique_state_count(),
                max_depth: self.max_depth(),
                out_degrees: self.out_degrees(),
                in_degrees: self.in_degrees(),
                duration: method_start.elapsed(),
                done: false,
            });
            std::thread::sleep(std::time::Duration::from_millis(1_000));
        }
        reporter.report_checking(ReportData {
            total_states: self.state_count(),
            unique_states: self.unique_state_count(),
            max_depth: self.max_depth(),
            out_degrees: self.out_degrees(),
            in_degrees: self.in_degrees(),
            duration: method_start.elapsed(),
            done: true,
        });

        // Finish with a discovery summary.
        let mut discoveries = HashMap::new();
        for (name, path) in self.discoveries() {
            let discovery = ReportDiscovery {
                path,
                classification: self.discovery_classification(name),
            };
            discoveries.insert(name, discovery);
        }
        reporter.report_discoveries(discoveries);

        self
    }

    /// Indicates whether a discovery is an `"example"` or `"counterexample"`.
    fn discovery_classification(&self, name: &str) -> DiscoveryClassification {
        let properties = self.model().properties();
        let property = properties.iter().find(|p| p.name == name).unwrap();
        match property.expectation {
            Expectation::Always | Expectation::Eventually => {
                DiscoveryClassification::Counterexample
            }
            Expectation::Sometimes => DiscoveryClassification::Example,
        }
    }

    /// A helper that verifies examples exist for all `sometimes` properties and no counterexamples
    /// exist for any `always`/`eventually` properties.
    fn assert_properties(&self)
    where
        M::Action: Debug,
        M::State: Debug,
    {
        for p in self.model().properties() {
            match p.expectation {
                Expectation::Always => self.assert_no_discovery(p.name),
                Expectation::Eventually => self.assert_no_discovery(p.name),
                Expectation::Sometimes => {
                    self.assert_any_discovery(p.name);
                }
            }
        }
    }

    /// Panics if a particular discovery is not found.
    fn assert_any_discovery(&self, name: &'static str) -> Path<M::State, M::Action> {
        if let Some(found) = self.discovery(name) {
            return found;
        }
        assert!(
            self.is_done(),
            "Discovery for \"{}\" not found, but model checking is incomplete.",
            name
        );
        panic!("Discovery for \"{}\" not found.", name);
    }

    /// Panics if a particular discovery is found.
    fn assert_no_discovery(&self, name: &'static str)
    where
        M::Action: Debug,
        M::State: Debug,
    {
        if let Some(found) = self.discovery(name) {
            panic!(
                "Unexpected \"{}\" {} {}Last state: {:?}\n",
                name,
                self.discovery_classification(name),
                found,
                found.last_state()
            );
        }
        assert!(
            self.is_done(),
            "Discovery for \"{}\" not found, but model checking is incomplete.",
            name
        );
    }

    /// Panics if the specified actions do not result in a discovery for the specified property
    /// name.
    fn assert_discovery(&self, name: &'static str, actions: Vec<M::Action>)
    where
        M::State: Debug + PartialEq,
        M::Action: Debug + PartialEq,
    {
        let mut additional_info: Vec<&'static str> = Vec::new();

        let found = self.assert_any_discovery(name);
        for init_state in self.model().init_states() {
            if let Some(path) = Path::from_actions(self.model(), init_state, &actions) {
                let property = self.model().property(name);
                match property.expectation {
                    Expectation::Always => {
                        if !(property.condition)(self.model(), path.last_state()) {
                            return;
                        }
                    }
                    Expectation::Eventually => {
                        let states = path.into_states();
                        let is_liveness_satisfied =
                            states.iter().any(|s| (property.condition)(self.model(), s));
                        let is_path_terminal = {
                            let mut actions = Vec::new();
                            self.model().actions(states.last().unwrap(), &mut actions);
                            actions.is_empty()
                        };
                        if !is_liveness_satisfied && is_path_terminal {
                            return;
                        }
                        if is_liveness_satisfied {
                            additional_info
                                .push("incorrect counterexample satisfies eventually property");
                        }
                        if !is_path_terminal {
                            additional_info.push("incorrect counterexample is nonterminal");
                        }
                    }
                    Expectation::Sometimes => {
                        if (property.condition)(self.model(), path.last_state()) {
                            return;
                        }
                    }
                }
            }
        }
        let additional_info = if additional_info.is_empty() {
            "".to_string()
        } else {
            format!(" ({})", additional_info.join("; "))
        };
        panic!(
            "Invalid discovery for \"{}\"{}, but a valid one was found. found={:?}",
            name,
            additional_info,
            found.into_actions()
        );
    }
}

// EventuallyBits tracks one bit per 'eventually' property being checked. Properties are assigned
// bit-numbers just by counting the 'eventually' properties up from 0 in the properties list. If a
// bit is present in a bitset, the property has _not_ been found on this path yet. Bits are removed
// from the propagating bitset when we find a state satisfying an `eventually` property; these
// states are not considered discoveries. Only if we hit the "end" of a path (i.e. return to a known
// state / no further state) with any of these bits still 1, the path is considered a discovery,
// a counterexample to the property.
type EventuallyBits = id_set::IdSet;

#[cfg(test)]
mod test_eventually_property_checker {
    use crate::test_util::dgraph::DGraph;
    use crate::{Checker, Property};

    fn eventually_odd() -> Property<DGraph> {
        Property::eventually("odd", |_, s| s % 2 == 1)
    }

    #[test]
    fn can_validate() {
        DGraph::with_property(eventually_odd())
            .with_path(vec![1]) // satisfied at terminal init
            .with_path(vec![2, 3]) // satisfied at nonterminal init
            .with_path(vec![2, 6, 7]) // satisfied at terminal next
            .with_path(vec![4, 9, 10]) // satisfied at nonterminal next
            .check()
            .assert_properties();
        // Repeat with distinct state spaces since stateful checking skips visited states (which we
        // don't expect here, but this is defense in depth).
        DGraph::with_property(eventually_odd())
            .with_path(vec![1])
            .check()
            .assert_properties();
        DGraph::with_property(eventually_odd())
            .with_path(vec![2, 3])
            .check()
            .assert_properties();
        DGraph::with_property(eventually_odd())
            .with_path(vec![2, 6, 7])
            .check()
            .assert_properties();
        DGraph::with_property(eventually_odd())
            .with_path(vec![4, 9, 10])
            .check()
            .assert_properties();
    }

    #[test]
    fn can_discover_counterexample() {
        // i.e. can falsify
        assert_eq!(
            DGraph::with_property(eventually_odd())
                .with_path(vec![0, 1])
                .with_path(vec![0, 2])
                .check()
                .discovery("odd")
                .unwrap()
                .into_states(),
            vec![0, 2]
        );
        assert_eq!(
            DGraph::with_property(eventually_odd())
                .with_path(vec![0, 1])
                .with_path(vec![2, 4])
                .check()
                .discovery("odd")
                .unwrap()
                .into_states(),
            vec![2, 4]
        );
        assert_eq!(
            DGraph::with_property(eventually_odd())
                .with_path(vec![0, 1, 4, 6])
                .with_path(vec![2, 4, 8])
                .check()
                .discovery("odd")
                .unwrap()
                .into_states(),
            vec![2, 4, 6]
        );
    }

    #[test]
    fn fixme_can_miss_counterexample_when_revisiting_a_state() {
        // i.e. incorrectly verify
        assert_eq!(
            DGraph::with_property(eventually_odd())
                .with_path(vec![0, 2, 4, 2]) // cycle
                .check()
                .discovery("odd"),
            None
        ); // FIXME: `unwrap().into_states()` should be [0, 2, 4, 2]
        assert_eq!(
            DGraph::with_property(eventually_odd())
                .with_path(vec![0, 2, 4])
                .with_path(vec![1, 4, 6]) // revisiting 4
                .check()
                .discovery("odd"),
            None
        ); // FIXME: `unwrap().into_states()` should be [0, 2, 4, 6]
    }
}

#[cfg(test)]
mod test_path {
    use super::*;
    use crate::fingerprint;
    use crate::test_util::linear_equation_solver::LinearEquation;
    use std::collections::VecDeque;

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
        assert_eq!(path.last_state(), &(2, 1));
        assert_eq!(
            path.last_state(),
            &Path::final_state(&model, fingerprints).unwrap()
        );
    }
}

#[cfg(test)]
mod test_report {
    use super::*;
    use crate::{report::WriteReporter, test_util::linear_equation_solver::LinearEquation};

    #[test]
    fn report_includes_property_names_and_paths() {
        // The assertions use `starts_with` to omit timing since it varies.

        // BFS
        let mut written: Vec<u8> = Vec::new();
        LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_bfs()
            .report(&mut WriteReporter::new(&mut written));
        let output = String::from_utf8(written).unwrap();
        assert!(
            output.starts_with(
                "\
                Checking. states=1, unique=1, depth=0, avg out degree=0, avg in degree=0\n\
                Done. states=15, unique=12, depth=4, avg out degree=1.1666666666666667, avg in degree=1.1666666666666667, sec="
            ),
            "Output did not start as expected (see test). output={:?}`",
            output
        );
        assert!(
            output.ends_with(
                "\
                Discovered \"solvable\" example Path[3]:\n\
                - IncreaseX\n\
                - IncreaseX\n\
                - IncreaseY\n"
            ),
            "Output did not end as expected (see test). output={:?}`",
            output
        );

        // DFS
        let mut written: Vec<u8> = Vec::new();
        LinearEquation { a: 2, b: 10, c: 14 }
            .checker()
            .spawn_dfs()
            .report(&mut WriteReporter::new(&mut written));
        let output = String::from_utf8(written).unwrap();
        assert!(
            output.starts_with(
                "\
                Checking. states=1, unique=1, depth=0, avg out degree=0, avg in degree=0\n\
                Done. states=55, unique=55, depth=28, avg out degree=0.9818181818181818, avg in degree=0.9818181818181818, sec="
            ),
            "Output did not start as expected (see test). output={:?}`",
            output
        );
        assert!(
            output.ends_with(
                "\
                Discovered \"solvable\" example Path[27]:\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n\
                - IncreaseY\n"
            ),
            "Output did not end as expected (see test). output={:?}`",
            output
        );
    }
}
