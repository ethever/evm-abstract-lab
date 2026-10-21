//! Finite, congruence and nonzero exchange with bounded transactional facts.
use super::{
    Accepts, Bounds, BoundsRoute, Charges, DomainSpec, FactError, KernelError, LocalClosure,
    MaskRoute, Masks, NumericValue, PublishError, Read, Reduction, RuleError, ScalarGuarantees,
    ScalarRoute, Scalars, Scope, WorkExhausted, publish, read,
};
use crate::domain::{EvmValue, WordElement};

pub(super) struct ScalarRule {
    domain: DomainSpec,
    lattice: crate::domain::facts::FactLattice,
    pub(super) value: NumericValue,
    pub(super) rounds: usize,
    pub(super) strengthened: usize,
}
impl ScalarRule {
    pub(super) fn new(domain: DomainSpec, value: NumericValue) -> Self {
        Self {
            domain,
            lattice: crate::domain::facts::FactLattice::new(domain.fact_limit()),
            value,
            rounds: 0,
            strengthened: 0,
        }
    }
}
impl<Cx> Reduction<Cx> for ScalarRule
where
    Cx: Read<Masks, MaskRoute, Error = KernelError<WorkExhausted>>
        + Read<Bounds, BoundsRoute, Error = KernelError<WorkExhausted>>
        + Read<Scalars, ScalarRoute, Error = KernelError<WorkExhausted>>
        + Accepts<Masks, MaskRoute, Error = PublishError<FactError, WorkExhausted>>
        + Accepts<Bounds, BoundsRoute, Error = PublishError<FactError, WorkExhausted>>
        + Accepts<Scalars, ScalarRoute, Error = PublishError<FactError, WorkExhausted>>
        + Charges<Error = KernelError<WorkExhausted>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        let Some(bits) = read::<Masks, _, MaskRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        let Some(interval) = read::<Bounds, _, BoundsRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        let Some(scalar) = read::<Scalars, _, ScalarRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        let value = NumericValue {
            bits,
            interval,
            finite: scalar.finite,
            congruence: scalar.congruence,
            nonzero: scalar.nonzero,
        };
        cx.charge(
            4096usize
                .saturating_add(256usize.saturating_mul(self.domain.capacity()))
                .saturating_add(32usize.saturating_mul(self.domain.capacity().saturating_add(32))),
        )?;
        super::super::reduce::export(
            self.domain,
            &EvmValue::from_numeric(value.clone()),
            &mut self.lattice,
            &mut self.strengthened,
        )?;
        let observation = self
            .lattice
            .scalar(super::super::facts::Symbol::THIS)
            .map(super::super::WordObservation::from_scalar)
            .transpose()?;
        let next = match observation {
            Some(o) => value.constrain(&o)?,
            None => value,
        };
        publish::<Masks, _, MaskRoute>(cx, *scope, next.bits)?;
        publish::<Bounds, _, BoundsRoute>(cx, *scope, next.interval)?;
        publish::<Scalars, _, ScalarRoute>(
            cx,
            *scope,
            ScalarGuarantees {
                finite: next.finite.clone(),
                congruence: next.congruence.clone(),
                nonzero: next.nonzero,
            },
        )?;
        self.value = next;
        self.rounds += 1;
        Ok(LocalClosure::Stable)
    }
}
