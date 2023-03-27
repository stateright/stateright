//! Private module for selective re-export. See [`LinearizabilityTester`].

use crate::semantics::{ConsistencyTester, SequentialSpec};
use std::collections::{btree_map, BTreeMap, VecDeque};
use std::fmt::Debug;

// This implementation is based on `SequentialConsistencyTester` and will be
// easier to follow if you are already familiar with that code. The key
// difference is that upon starting an operation, this tester also records
// the index of the last operation completed by every other thread, and those
// same indices are also preserved if/when the operation completes. That data
// allows the tester to reject histories that violate "real time" ordering.

/// This tester captures a potentially concurrent history of operations and
/// validates that it adheres to a [`SequentialSpec`] based on the
/// [linearizability] consistency model. This model requires that operations be
/// applied atomically and that sequenced (non-concurrent) operations are
/// applied in order.
///
/// If you're not sure whether to pick this or [`SequentialConsistencyTester`], favor
/// `LinearizabilityTester`.
///
/// # Linearizability
///
/// Unlike with [sequential consistency], all sequenced (non-concurrent) operations must respect a
/// happens-before relationship, even across threads, which ensures that histories respect "real
/// time" ordering (defined more precisely below).  Anomalies are prevented because threads all
/// agree on the viable order of operations.  For example, the later read by Thread 2 must read the
/// value of Thread 1's write (rather than a differing earlier value) since those two operations
/// are not concurrent:
///
/// ```text
///           -----------Time------------------------------>
/// Thread 1: [write invoked... and returns]
/// Thread 2:                                 [read invoked... and returns]
/// ```
///
/// While "real time" is a common way to phrase an implicit total ordering on non-concurrent events
/// spanning threads, a more precise way to think about this is that prior to Thread 2 starting its
/// read, Thread 1 is capable of communicating with Thread 2 indicating that the write finished.
/// This perspective avoids introducing the notion of a shared global time, which is often a
/// misleading perspective when it comes to distributed systems (or even modern physics in
/// general).
///
/// The [`SequentialSpec`] will imply additional ordering constraints based on semantics specific
/// to each operation. For example, a value cannot be popped off a stack before it is pushed. It is
/// then the responsibility of this tester to establish whether a valid total ordering of events
/// exists under these constraints.
///
/// See also: [`SequentialConsistencyTester`].
///
/// [linearizability]: https://en.wikipedia.org/wiki/Linearizability
/// [sequential consistency]: https://en.wikipedia.org/wiki/Sequential_consistency
/// [`SequentialConsistencyTester`]: crate::semantics::SequentialConsistencyTester
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(clippy::type_complexity)]
pub struct LinearizabilityTester<ThreadId, RefObj: SequentialSpec> {
    init_ref_obj: RefObj,
    history_by_thread: BTreeMap<ThreadId, VecDeque<Complete<ThreadId, RefObj::Op, RefObj::Ret>>>,
    in_flight_by_thread: BTreeMap<ThreadId, InFlight<ThreadId, RefObj::Op>>,
    is_valid_history: bool,
}

type LastCompletedOpMap<ThreadId> = BTreeMap<ThreadId, usize>;
type Complete<ThreadId, Op, Ret> = (LastCompletedOpMap<ThreadId>, Op, Ret);
type InFlight<ThreadId, Op> = (LastCompletedOpMap<ThreadId>, Op);

#[allow(clippy::len_without_is_empty)] // no use case for an emptiness check
impl<T: Ord, RefObj: SequentialSpec> LinearizabilityTester<T, RefObj> {
    /// Constructs a [`LinearizabilityTester`].
    pub fn new(init_ref_obj: RefObj) -> Self {
        Self {
            init_ref_obj,
            history_by_thread: Default::default(),
            in_flight_by_thread: Default::default(),
            is_valid_history: true,
        }
    }

    /// Indicates the aggregate number of operations completed or in flight
    /// across all threads.
    pub fn len(&self) -> usize {
        let mut len = self.in_flight_by_thread.len();
        for history in self.history_by_thread.values() {
            len += history.len();
        }
        len
    }
}

impl<T, RefObj> ConsistencyTester<T, RefObj> for LinearizabilityTester<T, RefObj>
where
    T: Copy + Debug + Ord,
    RefObj: Clone + SequentialSpec,
    RefObj::Op: Clone + Debug,
    RefObj::Ret: Clone + Debug + PartialEq,
{
    /// Indicates that a thread invoked an operation. Returns `Ok(...)` if the
    /// history is valid, even if it is not lineariable.
    ///
    /// See [`LinearizabilityTester::serialized_history`].
    fn on_invoke(&mut self, thread_id: T, op: RefObj::Op) -> Result<&mut Self, String> {
        if !self.is_valid_history {
            return Err("Earlier history was invalid.".to_string());
        }
        let in_flight_elem = self.in_flight_by_thread.entry(thread_id);
        if let btree_map::Entry::Occupied(occupied_op_entry) = in_flight_elem {
            self.is_valid_history = false;
            let (_, op) = occupied_op_entry.get();
            return Err(format!(
                    "Thread already has an operation in flight. thread_id={:?}, op={:?}, history_by_thread={:?}",
                    thread_id, op, self.history_by_thread));
        };
        let last_completed = self
            .history_by_thread
            .iter()
            .filter_map(|(id, cs)| {
                // collect last completed op index for every other thread
                if id == &thread_id || cs.is_empty() {
                    None
                } else {
                    Some((*id, cs.len() - 1))
                }
            })
            .collect::<BTreeMap<_, _>>();
        in_flight_elem.or_insert((last_completed, op));
        self.history_by_thread
            .entry(thread_id)
            .or_insert_with(VecDeque::new); // `serialize` requires entry
        Ok(self)
    }

    /// Indicates that a thread's earlier operation invocation returned. Returns
    /// `Ok(...)` if the history is valid, even if it is not linearizable.
    ///
    /// See [`LinearizabilityTester::serialized_history`].
    fn on_return(&mut self, thread_id: T, ret: RefObj::Ret) -> Result<&mut Self, String> {
        if !self.is_valid_history {
            return Err("Earlier history was invalid.".to_string());
        }
        let (completed, op) = match self.in_flight_by_thread.remove(&thread_id) {
            None => {
                self.is_valid_history = false;
                return Err(format!(
                    "There is no in-flight invocation for this thread ID. \
                     thread_id={:?}, unexpected_return={:?}, history={:?}",
                    thread_id,
                    ret,
                    self.history_by_thread
                        .entry(thread_id)
                        .or_insert_with(VecDeque::new)
                ));
            }
            Some(x) => x,
        };
        self.history_by_thread
            .entry(thread_id)
            .or_insert_with(VecDeque::new)
            .push_back((completed, op, ret));
        Ok(self)
    }

    /// Indicates whether the recorded history is linearizable.
    fn is_consistent(&self) -> bool {
        self.serialized_history().is_some()
    }
}

impl<T, RefObj> LinearizabilityTester<T, RefObj>
where
    T: Copy + Debug + Ord,
    RefObj: Clone + SequentialSpec,
    RefObj::Op: Clone + Debug,
    RefObj::Ret: Clone + Debug + PartialEq,
{
    /// Attempts to serialize the recorded partially ordered operation history
    /// into a total order that is consistent with a reference object's
    /// operational semantics.
    pub fn serialized_history(&self) -> Option<Vec<(RefObj::Op, RefObj::Ret)>> {
        if !self.is_valid_history {
            return None;
        }
        let history_by_thread = self
            .history_by_thread
            .iter()
            .map(|(t, cs)| (*t, cs.clone().into_iter().enumerate().collect()))
            .collect();
        Self::serialize(
            Vec::new(),
            &self.init_ref_obj,
            &history_by_thread,
            &self.in_flight_by_thread,
        )
    }

    #[allow(clippy::type_complexity)]
    fn serialize(
        valid_history: Vec<(RefObj::Op, RefObj::Ret)>, // total order
        ref_obj: &RefObj,
        remaining_history_by_thread: &BTreeMap<
            T,
            VecDeque<(usize, Complete<T, RefObj::Op, RefObj::Ret>)>,
        >, // partial order
        in_flight_by_thread: &BTreeMap<T, InFlight<T, RefObj::Op>>,
    ) -> Option<Vec<(RefObj::Op, RefObj::Ret)>> {
        // Return collected total order when there is no remaining partial order to interleave.
        let done = remaining_history_by_thread
            .iter()
            .all(|(_id, h)| h.is_empty());
        if done {
            return Some(valid_history);
        }

        // Otherwise try remaining interleavings.
        for (thread_id, remaining_history) in remaining_history_by_thread.iter() {
            let mut remaining_history_by_thread =
                std::borrow::Cow::Borrowed(remaining_history_by_thread);
            let mut in_flight_by_thread = std::borrow::Cow::Borrowed(in_flight_by_thread);
            let (ref_obj, valid_history) = if remaining_history.is_empty() {
                // Case 1: No remaining history to interleave. Maybe in-flight.
                if !in_flight_by_thread.contains_key(thread_id) {
                    continue;
                }
                let (cs, op) = in_flight_by_thread.to_mut().remove(thread_id).unwrap(); // `contains_key` above
                let violation = cs.iter().any(|(peer_id, min_peer_time)| {
                    // Ensure all pre-req operations were completed by peers
                    if let Some(ops) = remaining_history_by_thread.get(peer_id) {
                        if let Some((next_peer_time, _)) = ops.iter().next() {
                            if next_peer_time <= min_peer_time {
                                return true;
                            }
                        }
                    }
                    false
                });
                if violation {
                    continue;
                }
                let mut ref_obj = ref_obj.clone();
                let ret = ref_obj.invoke(&op);
                let mut valid_history = valid_history.clone();
                valid_history.push((op, ret));
                (ref_obj, valid_history)
            } else {
                // Case 2: Has remaining history to interleave.
                let (_t, (cs, op, ret)) = remaining_history_by_thread
                    .to_mut()
                    .get_mut(thread_id)
                    .unwrap() // iterator returned this thread ID
                    .pop_front()
                    .unwrap(); // `!is_empty()` above
                let violation = cs.iter().any(|(peer_id, min_peer_time)| {
                    // Ensure all pre-req operations were completed by peers
                    if let Some(ops) = remaining_history_by_thread.get(peer_id) {
                        if let Some((next_peer_time, _)) = ops.iter().next() {
                            if next_peer_time <= min_peer_time {
                                return true;
                            }
                        }
                    }
                    false
                });
                if violation {
                    continue;
                }
                let mut ref_obj = ref_obj.clone();
                if !ref_obj.is_valid_step(&op, &ret) {
                    continue;
                }
                let mut valid_history = valid_history.clone();
                valid_history.push((op, ret));
                (ref_obj, valid_history)
            };
            if let Some(valid_history) = Self::serialize(
                valid_history,
                &ref_obj,
                &remaining_history_by_thread,
                &in_flight_by_thread,
            ) {
                return Some(valid_history);
            }
        }
        None
    }
}

impl<T: Ord, RefObj> Default for LinearizabilityTester<T, RefObj>
where
    RefObj: Default + SequentialSpec,
{
    fn default() -> Self {
        Self::new(RefObj::default())
    }
}

impl<T, RefObj> serde::Serialize for LinearizabilityTester<T, RefObj>
where
    RefObj: serde::Serialize + SequentialSpec,
    RefObj::Op: serde::Serialize,
    RefObj::Ret: serde::Serialize,
    T: Ord + serde::Serialize,
{
    fn serialize<Ser: serde::Serializer>(&self, ser: Ser) -> Result<Ser::Ok, Ser::Error> {
        use serde::ser::SerializeStruct;
        let mut out = ser.serialize_struct("LinearizabilityTester", 4)?;
        out.serialize_field("init_ref_obj", &self.init_ref_obj)?;
        out.serialize_field("history_by_thread", &self.history_by_thread)?;
        out.serialize_field("in_flight_by_thread", &self.in_flight_by_thread)?;
        out.serialize_field("is_valid_history", &self.is_valid_history)?;
        out.end()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::semantics::register::*;
    use crate::semantics::vec::*;

    #[test]
    fn rejects_invalid_history() -> Result<(), String> {
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invoke(99, RegisterOp::Write('B'))?
                .on_invoke(99, RegisterOp::Write('C')),
            Err("Thread already has an operation in flight. thread_id=99, op=Write('B'), history_by_thread={99: []}".to_string()));
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invret(99, RegisterOp::Write('B'), RegisterRet::WriteOk)?
                .on_invret(99, RegisterOp::Write('C'), RegisterRet::WriteOk)?
                .on_return(99, RegisterRet::WriteOk),
            Err("There is no in-flight invocation for this thread ID. \
                 thread_id=99, \
                 unexpected_return=WriteOk, \
                 history=[({}, Write('B'), WriteOk), ({}, Write('C'), WriteOk)]"
                .to_string())
        );
        Ok(())
    }

    #[test]
    fn identifies_linearizable_register_history() -> Result<(), String> {
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invoke(0, RegisterOp::Write('B'))?
                .on_invret(1, RegisterOp::Read, RegisterRet::ReadOk('A'))?
                .serialized_history(),
            Some(vec![(RegisterOp::Read, RegisterRet::ReadOk('A')),])
        );
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invoke(0, RegisterOp::Read)?
                .on_invoke(1, RegisterOp::Write('B'))?
                .on_return(0, RegisterRet::ReadOk('B'))?
                .serialized_history(),
            Some(vec![
                (RegisterOp::Write('B'), RegisterRet::WriteOk),
                (RegisterOp::Read, RegisterRet::ReadOk('B')),
            ])
        );
        Ok(())
    }

    #[test]
    fn identifies_unlinearizable_register_history() -> Result<(), String> {
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invret(0, RegisterOp::Read, RegisterRet::ReadOk('B'))?
                .serialized_history(),
            None
        );
        assert_eq!(
            LinearizabilityTester::new(Register('A'))
                .on_invret(0, RegisterOp::Read, RegisterRet::ReadOk('B'))?
                .on_invoke(1, RegisterOp::Write('B'))?
                .serialized_history(),
            None // SC but not lineariable
        );
        Ok(())
    }

    #[test]
    fn identifies_linearizable_vec_history() -> Result<(), String> {
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invoke(0, VecOp::Push(10))?
                .serialized_history(),
            Some(vec![])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invoke(0, VecOp::Push(10))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(None))?
                .serialized_history(),
            Some(vec![(VecOp::Pop, VecRet::PopOk(None)),])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invoke(0, VecOp::Push(10))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(10)))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Pop, VecRet::PopOk(Some(10))),
            ])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(0, VecOp::Push(20))?
                .on_invret(1, VecOp::Len, VecRet::LenOk(1))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(20)))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(10)))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Len, VecRet::LenOk(1)),
                (VecOp::Push(20), VecRet::PushOk),
                (VecOp::Pop, VecRet::PopOk(Some(20))),
                (VecOp::Pop, VecRet::PopOk(Some(10))),
            ])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(0, VecOp::Push(20))?
                .on_invret(1, VecOp::Len, VecRet::LenOk(1))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(10)))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(20)))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Len, VecRet::LenOk(1)),
                (VecOp::Pop, VecRet::PopOk(Some(10))),
                (VecOp::Push(20), VecRet::PushOk),
                (VecOp::Pop, VecRet::PopOk(Some(20))),
            ])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(0, VecOp::Push(20))?
                .on_invret(1, VecOp::Len, VecRet::LenOk(2))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(20)))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(10)))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Push(20), VecRet::PushOk),
                (VecOp::Len, VecRet::LenOk(2)),
                (VecOp::Pop, VecRet::PopOk(Some(20))),
                (VecOp::Pop, VecRet::PopOk(Some(10))),
            ])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(1, VecOp::Len)?
                .on_invoke(0, VecOp::Push(20))?
                .on_return(1, VecRet::LenOk(1))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Len, VecRet::LenOk(1)),
            ])
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(1, VecOp::Len)?
                .on_invoke(0, VecOp::Push(20))?
                .on_return(1, VecRet::LenOk(2))?
                .serialized_history(),
            Some(vec![
                (VecOp::Push(10), VecRet::PushOk),
                (VecOp::Push(20), VecRet::PushOk),
                (VecOp::Len, VecRet::LenOk(2)),
            ])
        );
        Ok(())
    }

    #[test]
    fn identifies_unlinearizable_vec_history() -> Result<(), String> {
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(None))?
                .serialized_history(),
            None // SC but not lineariable
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(1, VecOp::Len)?
                .on_invoke(0, VecOp::Push(20))?
                .on_return(1, VecRet::LenOk(0))?
                .serialized_history(),
            None
        );
        assert_eq!(
            LinearizabilityTester::new(Vec::new())
                .on_invret(0, VecOp::Push(10), VecRet::PushOk)?
                .on_invoke(0, VecOp::Push(20))?
                .on_invret(1, VecOp::Len, VecRet::LenOk(2))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(10)))?
                .on_invret(1, VecOp::Pop, VecRet::PopOk(Some(20)))?
                .serialized_history(),
            None
        );
        Ok(())
    }
}
