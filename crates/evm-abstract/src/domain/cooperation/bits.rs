//! Interval provider -> mask consumer, with no access to the product carrier.
use super::{
    Accepts, Bounds, BoundsRoute, Charges, FactError, KernelError, KnownBits, LocalClosure,
    MaskRoute, Masks, PublishError, Read, Reduction, RuleError, Scope, WorkExhausted,
    contradiction, publish, read,
};

/// Infer fixed prefixes common to every signed/unsigned interval segment.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoundsToMask;
impl<Cx> Reduction<Cx> for BoundsToMask
where
    Cx: Read<Bounds, BoundsRoute, Error = KernelError<WorkExhausted>>
        + Accepts<Masks, MaskRoute, Error = PublishError<FactError, WorkExhausted>>
        + Charges<Error = KernelError<WorkExhausted>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        let Some(bounds) = read::<Bounds, _, BoundsRoute>(cx, *scope)? else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(256)?;
        let mut inferred = None;
        for (lo, hi) in bounds.segments() {
            let bits = KnownBits::from_unsigned_bounds(lo, hi);
            inferred = Some(inferred.map_or(bits, |old: KnownBits| old.join(&bits)));
        }
        publish::<Masks, _, MaskRoute>(cx, *scope, inferred.ok_or_else(contradiction)?)?;
        Ok(LocalClosure::Stable)
    }
}

/// Alternative mask component that retains existing masks without inference.
/// Useful for an explicit precision ablation through the same production algebra.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeepMasks;
impl<Cx> Reduction<Cx> for KeepMasks {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, _: &mut Cx) -> Result<LocalClosure, RuleError> {
        Ok(LocalClosure::Stable)
    }
}
