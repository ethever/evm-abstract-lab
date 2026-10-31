//! Mask and congruence providers -> interval consumer, via tracked queries.
use super::{
    Accepts, Bounds, BoundsRoute, Charges, FactError, Interval, KernelError, LocalClosure,
    MaskRoute, Masks, PublishError, Read, Reduction, RuleError, ScalarRoute, Scalars, Scope,
    WorkExhausted, contradiction, publish, read,
};

/// Find sound range endpoints satisfying the current mask/congruence cover.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaskToBounds;
impl<Cx> Reduction<Cx> for MaskToBounds
where
    Cx: Read<Masks, MaskRoute, Error = KernelError<WorkExhausted>>
        + Read<Bounds, BoundsRoute, Error = KernelError<WorkExhausted>>
        + Read<Scalars, ScalarRoute, Error = KernelError<WorkExhausted>>
        + Accepts<Bounds, BoundsRoute, Error = PublishError<FactError, WorkExhausted>>
        + Charges<Error = KernelError<WorkExhausted>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        let Some(bits) = read::<Masks, _, MaskRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        let Some(bounds) = read::<Bounds, _, BoundsRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        let Some(scalar) = read::<Scalars, _, ScalarRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(4096)?;
        let mut inferred = None;
        for (lo, hi) in bounds.segments() {
            if let Some((lo, hi)) = scalar
                .congruence
                .first_last(lo, hi)
                .and_then(|(lo, hi)| super::super::query::mask_bounds(lo, hi, bits))
            {
                let segment = Interval::new_unsigned(lo, hi).expect("ordered feasible endpoints");
                inferred = Some(inferred.map_or(segment, |old: Interval| old.join(&segment)));
            }
        }
        let (lo, hi) = bits.unsigned_bounds();
        let inferred = inferred
            .ok_or_else(contradiction)?
            .meet(&Interval::new_unsigned(lo, hi).expect("valid masks have ordered bounds"))
            .ok_or_else(contradiction)?;
        publish::<Bounds, _, BoundsRoute>(cx, *scope, inferred)?;
        Ok(LocalClosure::Stable)
    }
}

/// Alternative interval component that retains current bounds unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeepBounds;
impl<Cx> Reduction<Cx> for KeepBounds {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, _: &mut Cx) -> Result<LocalClosure, RuleError> {
        Ok(LocalClosure::Stable)
    }
}
