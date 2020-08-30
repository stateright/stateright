//! Implements [`SequentialSpec`] for [`Register`] operational semantics.

use std::fmt::Debug;
use super::SequentialSpec;

/// A simple register used to define reference operational semantics via
/// [`SequentialSpec`].
#[derive(Clone, Default, Debug, Hash, PartialEq)]
pub struct Register<T>(pub T);

/// An operation that can be invoked upon a [`Register`], resulting in a
/// [`RegisterRet`]
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum RegisterOp<T> { Write(T), Read }

/// A return value for a [`RegisterOp`] invoked upon a [`Register`].
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum RegisterRet<T> { WriteOk, ReadOk(T) }

impl<T: Clone + Debug + PartialEq> SequentialSpec for Register<T> {
    type Op = RegisterOp<T>;
    type Ret = RegisterRet<T>;
    fn invoke(&mut self, op: &Self::Op) -> Self::Ret {
        match op {
            RegisterOp::Write(v) => {
                self.0 = v.clone();
                RegisterRet::WriteOk
            }
            RegisterOp::Read => RegisterRet::ReadOk(self.0.clone()),
        }
    }
    fn is_valid_step(&mut self, op: &Self::Op, ret: &Self::Ret) -> bool {
        // Override to avoid unnecessary `clone` on `Read`.
        match (op, ret) {
            (RegisterOp::Write(v), RegisterRet::WriteOk) => {
                self.0 = v.clone();
                true
            }
            (RegisterOp::Read, RegisterRet::ReadOk(v)) => {
                &self.0 == v
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn models_expected_semantics() {
        let mut r = Register('A');
        assert_eq!(r.invoke(&RegisterOp::Read),       RegisterRet::ReadOk('A'));
        assert_eq!(r.invoke(&RegisterOp::Write('B')), RegisterRet::WriteOk);
        assert_eq!(r.invoke(&RegisterOp::Read),       RegisterRet::ReadOk('B'));
    }

    #[test]
    fn accepts_valid_histories() {
        assert!(Register('A').is_valid_history(vec![]));
        assert!(Register('A').is_valid_history(vec![
            (RegisterOp::Read,       RegisterRet::ReadOk('A')),
            (RegisterOp::Write('B'), RegisterRet::WriteOk),
            (RegisterOp::Read,       RegisterRet::ReadOk('B')),
            (RegisterOp::Write('C'), RegisterRet::WriteOk),
            (RegisterOp::Read,       RegisterRet::ReadOk('C')),
        ]));
    }

    #[test]
    fn rejects_invalid_histories() {
        assert!(!Register('A').is_valid_history(vec![
            (RegisterOp::Read,       RegisterRet::ReadOk('B')),
            (RegisterOp::Write('B'), RegisterRet::WriteOk),
        ]));
        assert!(!Register('A').is_valid_history(vec![
            (RegisterOp::Write('B'), RegisterRet::WriteOk),
            (RegisterOp::Read,       RegisterRet::ReadOk('A')),
        ]));
    }
}
