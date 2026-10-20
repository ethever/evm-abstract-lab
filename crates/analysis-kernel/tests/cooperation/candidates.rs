//! Finite residue candidates; no interval provider state or scheduling is visible.
use super::{
    contracts::{Bounds, Candidates, Variable},
    support::{Conflict, RuleError, Stop},
};
use analysis_kernel::{
    Accepts, Charges, Here, KernelError, LocalClosure, PublishError, Read, Reduction, Scope,
    Scoped, There,
};

pub struct CandidateRule {
    pub import_bounds: bool,
    pub evaluations: usize,
}
impl<Cx> Reduction<Cx> for CandidateRule
where
    Cx: Read<Bounds, Here, Error = KernelError<Stop>>
        + Read<Candidates, There<Here>, Error = KernelError<Stop>>
        + Accepts<Candidates, There<Here>, Error = PublishError<Conflict, Stop>>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.evaluations += 1;
        if !self.import_bounds {
            return Ok(LocalClosure::Stable);
        }
        cx.charge(64)?;
        for variable in [Variable::X, Variable::Y] {
            let bound = <Cx as Read<Bounds>>::query(cx, Scoped::new(*scope, variable))?;
            let own =
                <Cx as Read<Candidates, There<Here>>>::query(cx, Scoped::new(*scope, variable))?;
            if let (Some(bound), Some(own)) = (bound.value(), own.value()) {
                let mask = (0..=10)
                    .filter(|v| bound.contains(*v))
                    .fold(0u16, |mask, v| mask | (1 << v));
                <Cx as Accepts<Candidates, There<Here>>>::accept(
                    cx,
                    Scoped::new(*scope, variable),
                    own & mask,
                )?;
            }
        }
        Ok(LocalClosure::Stable)
    }
}
