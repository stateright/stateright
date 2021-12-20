//! Private module for selective re-export.

/// This trait is used to reduce the state space when checking a model with
/// [`CheckerBuilder::spawn_sym`]. The trait indicates the ability to generate a representative
/// from a symmetry equivalence class for each state in [`Model::State`].
///
/// # How to Implement
///
/// Nontrivial implementations will typically leverage [`Reindex`] and [`Rewrite`]. Both take a
/// mapping from old to new, but they are used for different scenarios:
///
/// - Imagine you have two vectors indexed by process identifiers. You sort the first to arrive at
///   a representative mapping but must correct the other by reindexing.
/// - Imagine you have one vector indexed by process identifiers and another vector that
///   refers to a possible subset of process identifiers (e.g. this might indicate "crashed
///   processes"). You sort the first to arrive at a representative mapping but must correct the
///   other by rewriting the crashed process identifiers.
///
/// For instance:
///
/// ```rust
/// use stateright::{Reindex, Representative, Rewrite};
/// # type ProcessHeap = char;
/// # type ProcessStack = char;
/// struct SystemState {
///   process_heaps: Vec<ProcessHeap>,   // assumes pids aren't stored in heaps
///   process_stacks: Vec<ProcessStack>, // assumes pids aren't stored in stacks
///   crashed_processes: Vec<usize>,
/// }
/// impl Representative for SystemState {
///     fn representative(&self) -> Self {
///         let mut combined = self.process_heaps.iter()
///             .enumerate()
///             .map(|(i, ph)| (ph, i))
///             .collect::<Vec<_>>();
///         combined.sort_unstable();
/// 
///         let mapping = combined.iter()
///             .map(|(_, i)| combined[*i].1)
///             .collect();
///         Self {
///             process_heaps: combined.into_iter()
///                 .map(|(ph, _)| ph.clone())
///                 .collect(),
///             process_stacks: self.process_stacks.reindex(&mapping),
///             crashed_processes: self.crashed_processes.rewrite(&mapping),
///         }
///     }
/// }
/// ```
///
/// # Lower Level Details
///
/// The above explains the difference between reindexing and rewriting based on use case. The
/// following example shows how the behavior of the two functions differs even for the same inputs.
///
/// ```rust
/// use stateright::{Reindex, Rewrite};
/// let mapping = vec![1, 2, 0];  // permutation whereby states were rotated left
/// assert_eq!(
///     // Imagine you have two vectors indexed by process identifiers. You sort the first to
///     // arrive at a representative mapping but must correct the other by reindexing: the value 
///     // at index 0 moves to index 2, etc.
///     vec![2, 1, 0].reindex(&mapping),
///     vec![1, 0, 2]);
/// assert_eq!(
///     // Imagine you have one vector indexed by process identifiers and another vector that
///     // refers to a possible subset of process identifiers (e.g. this might indicate "crashed
///     // processes"). You sort the first to arrive at a representative mapping but must correct
///     // the other by rewriting the crashed process identifiers: pid 0 becomes pid 1, etc.
///     vec![2, 1, 0].rewrite(&mapping),
///     vec![0, 2, 1]);
/// ```
///
/// For more background on symmetry reduction and representative states, you can see the paper
/// "[Symmetric Spin](https://link.springer.com/chapter/10.1007/10722468_1)" by Bošnački, Dams,
/// and Holenderski.
///
/// [`CheckerBuilder::spawn_sym`]: crate::CheckerBuilder::spawn_sym
/// [`Model::State`]: crate::Model::State
/// [`Reindex`]: crate::Reindex
/// [`Rewrite`]: crate::Rewrite
pub trait Representative {
    /// Generates a representative value in an equivalence class for `self`.
    fn representative(&self) -> Self;
}
