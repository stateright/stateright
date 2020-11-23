use crate::{Model, Path};
use std::sync::{Arc, Mutex};

/// A visitor to apply to every [`Path`] of the checked [`Model`].
///
/// Implementations include [`StateRecorder`] and
/// `impl<M: Model> `[`Fn`]`(Path<M::State, M::Action>)`.
///
/// # Example
///
/// ```
/// # use stateright::*; let model = ();
/// model.checker()
///     .visitor(|p: Path<_, _>| println!("\t{:?}", p.last_state()))
///     .spawn_dfs().join();
/// ```
pub trait CheckerVisitor<M: Model> {
    /// The method to apply to every [`Path`].
    fn visit(&self, path: Path<M::State, M::Action>);
}
impl<M, F> CheckerVisitor<M> for F
where M: Model,
      F: Fn(Path<M::State, M::Action>),
{
    fn visit(&self, path: Path<M::State, M::Action>) {
        self(path)
    }
}

/// A [`CheckerVisitor`] that records states evaluated by the model checker. Does not record
/// generated states that are still pending property evaluation.
///
/// # Example
///
/// ```
/// # use stateright::*; let model = ();
/// # let expected_states = vec![()];
/// let (recorder, accessor) = StateRecorder::new_with_accessor();
/// model.checker().visitor(recorder).spawn_dfs().join();
/// assert_eq!(accessor(), expected_states);
/// ```
pub struct StateRecorder<M: Model>(Arc<Mutex<Vec<M::State>>>);
impl<M> CheckerVisitor<M> for StateRecorder<M>
where M: Model,
      M::State: Clone,
{
    fn visit(&self, path: Path<M::State, M::Action>) {
        self.0.lock().unwrap().push(path.last_state().clone())
    }
}
impl<M> StateRecorder<M>
where M: Model,
      M::State: Clone,
{
    /// Instantiates a ([`StateRecorder`], accessor) pair.
    pub fn new_with_accessor() -> (Self, impl Fn() -> Vec<M::State>) {
        let recorder = Self(Arc::new(Mutex::new(Vec::new())));
        let accessor = { let r = Arc::clone(&recorder.0); move || r.lock().unwrap().clone() };
        (recorder, accessor)
    }
}
