//! Implements [`SequentialSpec`] for [`Vec`] operational semantics.

use crate::semantics::SequentialSpec;

/// An operation that can be invoked upon a [`Vec`], resulting in a
/// [`VecRet`].
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum VecOp<T> {
    Push(T),
    Pop,
    Len,
}

/// A return value for a [`VecOp`] invoked upon a [`Vec`].
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum VecRet<T> {
    PushOk,
    PopOk(Option<T>),
    LenOk(usize),
}

impl<T> SequentialSpec for Vec<T>
where
    T: Clone + PartialEq,
{
    type Op = VecOp<T>;
    type Ret = VecRet<T>;
    fn invoke(&mut self, op: &Self::Op) -> Self::Ret {
        match op {
            VecOp::Push(v) => {
                self.push(v.clone());
                VecRet::PushOk
            }
            VecOp::Pop => VecRet::PopOk(self.pop()),
            VecOp::Len => VecRet::LenOk(self.len()),
        }
    }
    fn is_valid_step(&mut self, op: &Self::Op, ret: &Self::Ret) -> bool {
        // Override to avoid unnecessary `clone` on `Pop`/`Len`.
        match (op, ret) {
            (VecOp::Push(v), VecRet::PushOk) => {
                self.push(v.clone());
                true
            }
            (VecOp::Pop, VecRet::PopOk(v)) => &self.pop() == v,
            (VecOp::Len, VecRet::LenOk(l)) => &self.len() == l,
            _ => false,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn models_expected_semantics() {
        let mut v = vec!['A'];
        assert_eq!(v.invoke(&VecOp::Len), VecRet::LenOk(1));
        assert_eq!(v.invoke(&VecOp::Push('B')), VecRet::PushOk);
        assert_eq!(v.invoke(&VecOp::Len), VecRet::LenOk(2));
        assert_eq!(v.invoke(&VecOp::Pop), VecRet::PopOk(Some('B')));
        assert_eq!(v.invoke(&VecOp::Len), VecRet::LenOk(1));
        assert_eq!(v.invoke(&VecOp::Pop), VecRet::PopOk(Some('A')));
        assert_eq!(v.invoke(&VecOp::Len), VecRet::LenOk(0));
        assert_eq!(v.invoke(&VecOp::Pop), VecRet::PopOk(None));
        assert_eq!(v.invoke(&VecOp::Len), VecRet::LenOk(0));
    }

    #[test]
    fn accepts_valid_histories() {
        assert!(Vec::<isize>::new().is_valid_history(vec![]));
        assert!(Vec::new().is_valid_history(vec![
            (VecOp::Push(10), VecRet::PushOk),
            (VecOp::Push(20), VecRet::PushOk),
            (VecOp::Len, VecRet::LenOk(2)),
            (VecOp::Pop, VecRet::PopOk(Some(20))),
            (VecOp::Len, VecRet::LenOk(1)),
            (VecOp::Pop, VecRet::PopOk(Some(10))),
            (VecOp::Len, VecRet::LenOk(0)),
            (VecOp::Pop, VecRet::PopOk(None)),
        ]));
    }

    #[test]
    fn rejects_invalid_histories() {
        assert!(!Vec::new().is_valid_history(vec![
            (VecOp::Push(10), VecRet::PushOk),
            (VecOp::Push(20), VecRet::PushOk),
            (VecOp::Len, VecRet::LenOk(1)),
            (VecOp::Push(30), VecRet::PushOk),
        ]));
        assert!(!Vec::new().is_valid_history(vec![
            (VecOp::Push(10), VecRet::PushOk),
            (VecOp::Push(20), VecRet::PushOk),
            (VecOp::Pop, VecRet::PopOk(Some(10))),
        ]));
    }
}
