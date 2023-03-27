//! Private module for selective re-export.

use crate::semantics::SequentialSpec;

/// An implementation of this trait tests the [consistency] of a concurrent system with respect to
/// a "reference sequential specification" [`SequentialSpec`]. The interface for doing so involves
/// recording operation invocations and returns.
///
/// Currently Stateright includes implementations in the form of a [`LinearizabilityTester`] and
/// [`SequentialConsistencyTester`].
///
/// [consistency]: https://en.wikipedia.org/wiki/Consistency_model
/// [`LinearizabilityTester`]: crate::semantics::LinearizabilityTester
/// [`SequentialConsistencyTester`]: crate::semantics::SequentialConsistencyTester
pub trait ConsistencyTester<T, RefObj>
where
    RefObj: SequentialSpec,
    T: Copy,
{
    /// Indicates that a thread invoked an operation. Returns `Ok(...)` if the
    /// history is valid, even if it is not consistent.
    fn on_invoke(&mut self, thread_id: T, op: RefObj::Op) -> Result<&mut Self, String>;

    /// Indicates that a thread's earlier operation invocation returned. Returns
    /// `Ok(...)` if the history is valid, even if it is not consistent.
    fn on_return(&mut self, thread_id: T, ret: RefObj::Ret) -> Result<&mut Self, String>;

    /// Indicates whether the recorded history is consistent with the semantics expected by the
    /// tester.
    fn is_consistent(&self) -> bool;

    /// A helper that indicates both an operation and corresponding return
    /// value for a thread. Returns `Ok(...)` if the history is valid, even if
    /// it is not consistent.
    fn on_invret(
        &mut self,
        thread_id: T,
        op: RefObj::Op,
        ret: RefObj::Ret,
    ) -> Result<&mut Self, String> {
        self.on_invoke(thread_id, op)?.on_return(thread_id, ret)
    }
}
