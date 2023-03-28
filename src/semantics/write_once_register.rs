//! Implements [`SequentialSpec`] for "write-once register" operational semantics.

use super::SequentialSpec;
use std::fmt::Debug;

/// A simple register used to define reference operational semantics via
/// [`SequentialSpec`].
#[derive(Clone, Default, Debug, Hash, PartialEq, serde::Serialize)]
pub struct WORegister<T>(pub Option<T>);

/// An operation that can be invoked upon a [`WORegister`], resulting in a
/// [`WORegisterRet`]
#[derive(Clone, Debug, Hash, PartialEq, serde::Serialize)]
pub enum WORegisterOp<T> {
    Write(T),
    Read,
}

/// A return value for a [`WORegisterOp`] invoked upon a [`WORegister`].
#[derive(Clone, Debug, Hash, PartialEq, serde::Serialize)]
pub enum WORegisterRet<T> {
    WriteOk,
    WriteFail,
    ReadOk(Option<T>),
}

impl<T: Clone + Debug + PartialEq> SequentialSpec for WORegister<T> {
    type Op = WORegisterOp<T>;
    type Ret = WORegisterRet<T>;
    fn invoke(&mut self, op: &Self::Op) -> Self::Ret {
        match (op, &self.0) {
            (WORegisterOp::Write(v), None) => {
                self.0 = Some(v.clone());
                WORegisterRet::WriteOk
            }
            // If something has already been written succeed if equal
            (WORegisterOp::Write(v), Some(vp)) if *v == *vp => {
                self.0 = Some(v.clone());
                WORegisterRet::WriteOk
            }
            (WORegisterOp::Write(_), Some(_)) => WORegisterRet::WriteFail,
            (WORegisterOp::Read, _) => WORegisterRet::ReadOk(self.0.clone()),
        }
    }
    fn is_valid_step(&mut self, op: &Self::Op, ret: &Self::Ret) -> bool {
        // Override to avoid unnecessary `clone` on `Read`.
        match (op, ret, &self.0) {
            (WORegisterOp::Write(v), WORegisterRet::WriteOk, None) => {
                self.0 = Some(v.clone());
                true
            }
            (WORegisterOp::Write(v), WORegisterRet::WriteOk, Some(vp)) if *v == *vp => true,
            (WORegisterOp::Write(v), WORegisterRet::WriteFail, Some(vp)) if *v != *vp => true,
            (WORegisterOp::Read, WORegisterRet::ReadOk(v), _) => &self.0 == v,
            _ => false,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn models_expected_semantics() {
        let mut r = WORegister(None);
        assert_eq!(r.invoke(&WORegisterOp::Write('A')), WORegisterRet::WriteOk);
        assert_eq!(
            r.invoke(&WORegisterOp::Read),
            WORegisterRet::ReadOk(Some('A'))
        );
        assert_eq!(
            r.invoke(&WORegisterOp::Write('B')),
            WORegisterRet::WriteFail
        );
        assert_eq!(
            r.invoke(&WORegisterOp::Read),
            WORegisterRet::ReadOk(Some('A'))
        );
    }

    #[test]
    fn accepts_valid_histories() {
        let none_char: Option<char> = None;
        assert!(WORegister(none_char).is_valid_history(vec![]));
        assert!(WORegister(none_char).is_valid_history(vec![
            (WORegisterOp::Read, WORegisterRet::ReadOk(None)),
            (WORegisterOp::Write('A'), WORegisterRet::WriteOk),
            (WORegisterOp::Read, WORegisterRet::ReadOk(Some('A'))),
            (WORegisterOp::Write('B'), WORegisterRet::WriteFail),
            (WORegisterOp::Read, WORegisterRet::ReadOk(Some('A'))),
            (WORegisterOp::Write('C'), WORegisterRet::WriteFail),
            (WORegisterOp::Read, WORegisterRet::ReadOk(Some('A'))),
        ]));
    }

    #[test]
    fn rejects_invalid_histories() {
        let none_char: Option<char> = None;
        assert!(!WORegister(Some('A')).is_valid_history(vec![
            (WORegisterOp::Read, WORegisterRet::ReadOk(Some('A'))),
            (WORegisterOp::Write('B'), WORegisterRet::WriteOk),
        ]));
        assert!(!WORegister(none_char).is_valid_history(vec![
            (WORegisterOp::Read, WORegisterRet::ReadOk(Some('A'))),
            (WORegisterOp::Write('A'), WORegisterRet::WriteOk),
        ]));
        assert!(!WORegister(none_char).is_valid_history(vec![
            (WORegisterOp::Read, WORegisterRet::ReadOk(None)),
            (WORegisterOp::Write('A'), WORegisterRet::WriteOk),
            (WORegisterOp::Write('B'), WORegisterRet::WriteOk),
        ]));
    }
}
