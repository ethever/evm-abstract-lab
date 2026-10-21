//! Observable rule wrappers shared by cooperation and product cost tests.

use super::{DomainSpec, RuleError, RuleWork, WorkExhausted};
use analysis_kernel::{Charges, KernelError, LocalClosure, Reduction, Scope};
use std::cell::Cell;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Count<'a, R> {
    pub rule: R,
    pub calls: &'a Cell<usize>,
}

impl<R: RuleWork> RuleWork for Count<'_, R> {
    fn evaluation_work(&self, spec: DomainSpec, finite: usize) -> usize {
        self.rule.evaluation_work(spec, finite)
    }
}

impl<Cx, R> Reduction<Cx> for Count<'_, R>
where
    R: Reduction<Cx, Task = Scope, Error = RuleError>,
{
    type Task = Scope;
    type Error = RuleError;

    fn reduce(&mut self, task: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.calls.set(self.calls.get() + 1);
        self.rule.reduce(task, cx)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CancelDuringRule<'a> {
    pub owner: &'a analysis_control::Cancellation,
    pub unrelated: &'a analysis_control::Cancellation,
    pub calls: &'a Cell<usize>,
    pub refused: &'a Cell<bool>,
}

impl<Cx: Charges<Error = KernelError<WorkExhausted>>> Reduction<Cx> for CancelDuringRule<'_> {
    type Task = Scope;
    type Error = RuleError;

    fn reduce(&mut self, _: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.calls.set(self.calls.get() + 1);
        cx.charge(100_000)?;
        self.owner.cancel();
        let refusal = analysis_control::with_cancellation(self.unrelated, || cx.charge(1));
        self.refused.set(refusal.is_err());
        Ok(LocalClosure::Stable)
    }
}
