//! Affine interval propagation; the candidate provider is used only via queries.
use super::{
    contracts::{Bounds, Candidates, Interval, Variable},
    support::{Conflict, RuleError, Stop},
};
use analysis_kernel::{
    Accepts, Charges, Here, KernelError, LocalClosure, PublishError, Read, Reduction, Scope,
    Scoped, There,
};

pub struct BoundsRule {
    pub import_candidates: bool,
    pub evaluations: usize,
}
impl<Cx> Reduction<Cx> for BoundsRule
where
    Cx: Read<Bounds, Here, Error = KernelError<Stop>>
        + Read<Candidates, There<Here>, Error = KernelError<Stop>>
        + Accepts<Bounds, Here, Error = PublishError<Conflict, Stop>>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.evaluations += 1;
        cx.charge(128)?;
        let x = <Cx as Read<Bounds>>::query(cx, Scoped::new(*scope, Variable::X))?;
        let y = <Cx as Read<Bounds>>::query(cx, Scoped::new(*scope, Variable::Y))?;
        let (Some(x), Some(y)) = (x.value(), y.value()) else {
            return Ok(LocalClosure::Stable);
        };
        let mut next_x = Interval::new(x.lo.max(10u8.saturating_sub(y.hi)), x.hi.min(10 - y.lo));
        let mut next_y = Interval::new(y.lo.max(10u8.saturating_sub(x.hi)), y.hi.min(10 - x.lo));
        if self.import_candidates {
            for (variable, target) in [(Variable::X, &mut next_x), (Variable::Y, &mut next_y)] {
                let reply = <Cx as Read<Candidates, There<Here>>>::query(
                    cx,
                    Scoped::new(*scope, variable),
                )?;
                if let Some(mask) = reply.value() {
                    let low = (0..=10)
                        .find(|v| mask & (1 << v) != 0)
                        .expect("nonempty candidate guarantee");
                    let high = (0..=10).rev().find(|v| mask & (1 << v) != 0).unwrap();
                    target.lo = target.lo.max(low);
                    target.hi = target.hi.min(high);
                }
            }
        }
        <Cx as Accepts<Bounds>>::accept(cx, Scoped::new(*scope, Variable::X), next_x)?;
        <Cx as Accepts<Bounds>>::accept(cx, Scoped::new(*scope, Variable::Y), next_y)?;
        Ok(LocalClosure::Stable)
    }
}
