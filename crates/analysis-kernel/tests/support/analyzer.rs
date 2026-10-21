//! Stable callbacks for scheduler tests that supply no semantic analysis.
use crate::{Analyzer, LocalClosure};
use std::convert::Infallible;

pub struct Noop;
impl<Cx> Analyzer<Cx> for Noop {
    type Task = ();
    type Error = Infallible;
    fn evaluate(&mut self, _: &(), _: &mut Cx) -> Result<LocalClosure, Infallible> {
        Ok(LocalClosure::Stable)
    }
}
