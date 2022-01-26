//! Private module for selective re-export.

/// This trait is used to reduce the state space when checking a model with
/// [`CheckerBuilder::symmetry`]. The trait indicates the ability to generate a representative
/// from a symmetry equivalence class for each state in [`Model::State`].
///
/// Bošnački, Dams, and Holenderski provide a clarifying example in their paper
/// "[Symmetric Spin](https://link.springer.com/chapter/10.1007/10722468_1)":
///
/// > _"In order to grasp the idea of symmetry reduction, consider a mutual exclusion protocol based on
/// > on semaphores. The (im)possibility for processes to enter their critical sections will be
/// > similar regardless of their identities, since process identities (pids) play no role in the
/// > semaphore mechanism. More formally, the system state remains behaviorally
/// > equivalent under permutations of pids. During state-space exploration, when a
/// > state is visited that is the same, up to a permutation of pids, as some state that
/// > has already been visited, the search can be pruned."_
///
/// # How to Implement
///
/// Here is an example implementation:
///
/// ```
/// # use std::collections::BTreeSet;
/// use stateright::{Representative, Rewrite, RewritePlan};
/// use stateright::util::DenseNatMap;
///
/// struct SystemState {
///     process_states: DenseNatMap<Pid, ProcessState>,
///     time_slice_sequence: Vec<Pid>,
///     // ... etc ...
/// }
/// impl Representative for SystemState {
///     fn representative(&self) -> Self {
///         let plan = (&self.process_states).into();
///         Self {
///             process_states: self.process_states.rewrite(&plan),
///             time_slice_sequence: self.time_slice_sequence.rewrite(&plan),
///             // ... etc ...
///         }
///     }
/// }
///
/// # type Pid = stateright::actor::Id;
/// #
/// #[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
/// struct ProcessState {
///     program_counter: usize,
///     parent: Pid,
///     // ... etc ...
/// }
/// impl Rewrite<Pid> for ProcessState {
///     fn rewrite<S>(&self, plan: &RewritePlan<Pid,S>) -> Self {
///         Self {
///             program_counter: self.program_counter.rewrite(plan), // no-op (cannot contain `Pid`)
///             parent: self.parent.rewrite(plan),
///             // ... etc ...
///         }
///     }
/// }
/// ```
///
/// [`CheckerBuilder::symmetry`]: crate::CheckerBuilder::symmetry
/// [`Model::State`]: crate::Model::State
/// [`Rewrite`]: crate::Rewrite
pub trait Representative {
    /// Generates a representative value in an equivalence class for `self`.
    fn representative(&self) -> Self;
}
