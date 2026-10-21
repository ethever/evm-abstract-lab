//! Default product assembly over the shared typed dependency kernel.
//!
//! Each invocation owns a fresh epoch and one fixed numeric input. Nothing in
//! these channels escapes into a later invocation or a widened input. Bit/range
//! rules depend only on their public query capabilities; the scalar rule keeps
//! the remaining finite/congruence/nonzero reductions and their fact limits.

use super::{Domain, DomainSpec, NumericReduction, NumericValue, ReductionStatus, WorkExhausted};
use super::{
    FiniteConstantSet, congruence::Congruence, facts::FactError, interval::Interval,
    known_bits::KnownBits,
};
use analysis_kernel::{
    Accepts, Availability, Capability, Channel, Charges, Closure, Cons, Context, Descending,
    Either, Here, Kernel, KernelError, LocalClosure, Meter, Nil, Pair, PublishError, Read,
    Reduction, Registered, Scope, ScopeKey, Scoped, There,
};

mod bits;
mod bounds;
mod cost;
mod policy;
mod scalar;
#[cfg(test)]
mod tests;
mod work;
pub use bits::{BoundsToMask, KeepMasks};
pub use bounds::{KeepBounds, MaskToBounds};
pub use cost::{BoundedWork, ComposedWork, ProductWork, RuleWork};
pub use policy::ProductAlgebra;
pub use work::Precharged;

#[cfg(test)]
pub(super) static PROTOCOL_WORK: crate::test_instrumentation::Counter<(usize, usize, usize)> =
    crate::test_instrumentation::Counter::new((0, 0, 0));

/// Guaranteed fixed bits, independent of the range implementation.
pub struct Masks;
impl Capability for Masks {
    type Key = ();
    type Value = KnownBits;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &KnownBits) -> usize {
        8
    }
}
/// Guaranteed signed/unsigned interval, independent of the mask implementation.
pub struct Bounds;
impl Capability for Bounds {
    type Key = ();
    type Value = Interval;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Interval) -> usize {
        8
    }
}
/// Remaining product guarantees; masks and intervals live in other providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarGuarantees {
    /// Complete finite cover, or unconstrained finite component.
    pub finite: FiniteConstantSet,
    /// Ordinary Word256 congruence guarantee.
    pub congruence: Congruence,
    /// Whether zero is excluded.
    pub nonzero: bool,
}
/// Typed finite/congruence/nonzero query family.
pub struct Scalars;
impl Capability for Scalars {
    type Key = ();
    type Value = ScalarGuarantees;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &ScalarGuarantees) -> usize {
        value.finite.work_size().saturating_add(8)
    }
}

/// Scoped numerical contradiction or a distinct kernel/resource refusal.
#[derive(Debug)]
pub enum RuleError {
    /// A numerical fact operation proved a contradiction or hit its capacity.
    Fact(FactError),
    /// Infrastructure failure is never a proof of numerical bottom.
    Kernel(KernelError<WorkExhausted>),
}
impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fact(error) => error.fmt(f),
            Self::Kernel(error) => error.fmt(f),
        }
    }
}
// Keep typed payloads above; Error::source uses the default None, without dyn.
impl std::error::Error for RuleError {}
impl From<FactError> for RuleError {
    fn from(error: FactError) -> Self {
        Self::Fact(error)
    }
}
impl From<KernelError<WorkExhausted>> for RuleError {
    fn from(error: KernelError<WorkExhausted>) -> Self {
        Self::Kernel(error)
    }
}
impl From<PublishError<FactError, WorkExhausted>> for RuleError {
    fn from(error: PublishError<FactError, WorkExhausted>) -> Self {
        match error {
            PublishError::Policy(e) => Self::Fact(e),
            PublishError::Kernel(e) => Self::Kernel(e),
        }
    }
}

/// Explicit route to masks in the default static provider list.
pub type MaskRoute = Here;
/// Explicit route to bounds in the default static provider list.
pub type BoundsRoute = There<Here>;
/// Explicit route to remaining guarantees in the default provider list.
pub type ScalarRoute = There<There<Here>>;

fn read<Q, Cx, Path>(cx: &mut Cx, scope: Scope) -> Result<Option<Q::Value>, RuleError>
where
    Q: Capability<Key = ()>,
    Cx: Read<Q, Path, Error = KernelError<WorkExhausted>>,
{
    Ok(match cx.query(Scoped::new(scope, ()))?.state {
        Availability::Value(v) => Some(v),
        Availability::Pending | Availability::Absent => None,
    })
}
fn publish<Q, Cx, Path>(cx: &mut Cx, scope: Scope, value: Q::Value) -> Result<(), RuleError>
where
    Q: Capability<Key = ()>,
    Cx: Accepts<Q, Path, Error = PublishError<FactError, WorkExhausted>>,
{
    cx.accept(Scoped::new(scope, ()), value)?;
    Ok(())
}

macro_rules! meet_policy {
    ($name:ident,$value:ty,$cost:expr,$meet:expr) => {
        /// Descending meet for one separately owned scalar query family.
        pub struct $name;
        impl analysis_kernel::RefinePolicy for $name {
            type Value = $value;
            type Error = FactError;
            fn work(&self, a: &Self::Value, b: &Self::Value) -> usize {
                ($cost)(a, b)
            }
            fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
                a == b
            }
            fn refine(&self, a: &Self::Value, b: &Self::Value) -> Result<Self::Value, FactError> {
                ($meet)(a, b)
            }
        }
    };
}
fn contradiction() -> FactError {
    FactError::Contradiction {
        subject: super::facts::Symbol::THIS,
    }
}
meet_policy!(
    MaskMeet,
    KnownBits,
    |_: &KnownBits, _: &KnownBits| 16,
    |a: &KnownBits, b: &KnownBits| a.meet(b).ok_or_else(contradiction)
);
meet_policy!(
    BoundsMeet,
    Interval,
    |_: &Interval, _: &Interval| 128,
    |a: &Interval, b: &Interval| a.meet(b).ok_or_else(contradiction)
);
meet_policy!(
    ScalarMeet,
    ScalarGuarantees,
    |a: &ScalarGuarantees, b: &ScalarGuarantees| Scalars::value_work(a)
        .saturating_add(Scalars::value_work(b))
        .saturating_mul(64),
    |a: &ScalarGuarantees, b: &ScalarGuarantees| -> Result<ScalarGuarantees, FactError> {
        let congruence = a.congruence.meet(&b.congruence).ok_or_else(contradiction)?;
        let nonzero = a.nonzero || b.nonzero;
        let finite = a
            .finite
            .meet(&b.finite)
            .map_err(|_| contradiction())?
            .filter(|v| (!nonzero || v != alloy_primitives::U256::ZERO) && congruence.contains(v))
            .map_err(|_| contradiction())?;
        Ok(ScalarGuarantees {
            finite,
            congruence,
            nonzero,
        })
    }
);

/// Static provider assembly used by the product rule implementations.
pub type ProductChannels = Cons<
    Channel<Masks, Descending<MaskMeet>>,
    Cons<
        Channel<Bounds, Descending<BoundsMeet>>,
        Cons<Channel<Scalars, Descending<ScalarMeet>>, Nil>,
    >,
>;

/// Registered seed task, admitted exactly once in a fresh fixed-input scope.
pub struct SeedInput {
    scope: Scope,
    value: NumericValue,
}
/// Statically distinct seed/mask/range/scalar tasks; no shared fact enum.
pub type ProductTask = Either<SeedInput, Either<Scope, Either<Scope, Scope>>>;
/// Concrete context available to independently replaceable mask/range rules.
pub type ProductContext<'r, 'k> = Context<'r, 'k, ProductTask, Precharged, ProductChannels>;

struct Seed;
impl<Cx> Reduction<Cx> for Seed
where
    Cx: Accepts<Masks, MaskRoute, Error = PublishError<FactError, WorkExhausted>>
        + Accepts<Bounds, BoundsRoute, Error = PublishError<FactError, WorkExhausted>>
        + Accepts<Scalars, ScalarRoute, Error = PublishError<FactError, WorkExhausted>>,
{
    type Task = SeedInput;
    type Error = RuleError;
    fn reduce(&mut self, t: &SeedInput, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        publish::<Masks, _, MaskRoute>(cx, t.scope, t.value.bits)?;
        publish::<Bounds, _, BoundsRoute>(cx, t.scope, t.value.interval)?;
        publish::<Scalars, _, ScalarRoute>(
            cx,
            t.scope,
            ScalarGuarantees {
                finite: t.value.finite.clone(),
                congruence: t.value.congruence.clone(),
                nonzero: t.value.nonzero,
            },
        )?;
        Ok(LocalClosure::Stable)
    }
}

pub(super) fn close(domain: Domain, value: NumericValue) -> NumericReduction<NumericValue> {
    // This convergence proof belongs only to this exact built-in assembly.
    let work = domain.numeric_refinement_work(&value, 0);
    close_with_work(domain, value, BoundsToMask, MaskToBounds, work)
}

#[cfg(test)]
fn close_order<M, R>(
    domain: Domain,
    value: NumericValue,
    masks: M,
    bounds: R,
    mask_first: bool,
) -> NumericReduction<NumericValue>
where
    M: Copy + RuleWork,
    R: Copy + RuleWork,
    for<'r, 'k> M: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
    for<'r, 'k> R: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
{
    let work = ComposedWork.closure_work(
        domain.spec(),
        &masks,
        &bounds,
        domain.capacity().max(value.finite.work_size()),
    );
    close_order_with_work(domain, value, masks, bounds, mask_first, work)
}

pub(super) fn close_with_work<M, R>(
    domain: Domain,
    value: NumericValue,
    masks: M,
    bounds: R,
    work: usize,
) -> NumericReduction<NumericValue>
where
    M: Copy,
    R: Copy,
    for<'r, 'k> M: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
    for<'r, 'k> R: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
{
    close_order_with_work(domain, value, masks, bounds, true, work)
}

fn close_order_with_work<M, R>(
    domain: Domain,
    value: NumericValue,
    masks: M,
    bounds: R,
    mask_first: bool,
    work: usize,
) -> NumericReduction<NumericValue>
where
    M: Copy,
    R: Copy,
    for<'r, 'k> M: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
    for<'r, 'k> R: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
{
    if analysis_control::current_cancellation().is_cancelled() {
        return limited(value, ReductionStatus::Cancelled);
    }
    let Some(cap) = domain
        .spec()
        .reduction_rounds()
        .checked_mul(3)
        .and_then(|steps| steps.checked_add(1))
        .filter(|_| work != usize::MAX)
    else {
        return limited(value, ReductionStatus::WorkLimit);
    };
    // All semantic work and dependency allocation was reserved at the facade.
    let mut paid = Precharged::new(work);
    if let Err(error) = paid.checkpoint() {
        return limited(value, paid.failure_status(&KernelError::Stopped(error)));
    }
    #[cfg(test)]
    let prepaid = paid.remaining();
    let mut kernel = match Kernel::<ProductTask>::new() {
        Ok(k) => k,
        Err(_) => return limited(value, ReductionStatus::WorkLimit),
    };
    let setup = (|| {
        let scope = kernel.scope(
            ScopeKey {
                context: 0,
                generation: 0,
                policy: 0,
            },
            &mut paid,
        )?;
        let a = kernel.channel::<Masks, _, _>(Descending(MaskMeet), &mut paid)?;
        let b = kernel.channel::<Bounds, _, _>(Descending(BoundsMeet), &mut paid)?;
        let c = kernel.channel::<Scalars, _, _>(Descending(ScalarMeet), &mut paid)?;
        kernel.register(
            scope,
            Either::Left(SeedInput {
                scope,
                value: value.clone(),
            }),
            &mut paid,
        )?;
        let mask = Either::Right(Either::Left(scope));
        let bound = Either::Right(Either::Right(Either::Left(scope)));
        let initial = if mask_first {
            [mask, bound]
        } else {
            [bound, mask]
        };
        for task in initial {
            kernel.register(scope, task, &mut paid)?;
        }
        kernel.register(
            scope,
            Either::Right(Either::Right(Either::Right(scope))),
            &mut paid,
        )?;
        kernel.seal_scope(scope, &mut paid)?;
        Ok::<_, KernelError<WorkExhausted>>(Cons::new(a, Cons::new(b, Cons::new(c, Nil))))
    })();
    let mut channels = match setup {
        Ok(channels) => channels,
        Err(error) => return limited(value, paid.failure_status(&error)),
    };
    let mut rules = Pair::new(
        Registered::new(Seed),
        Pair::new(
            Registered::new(masks),
            Pair::new(
                Registered::new(bounds),
                Registered::new(scalar::ScalarRule::new(domain, value)),
            ),
        ),
    );
    let result = analysis_kernel::solve(&mut kernel, &mut rules, &mut channels, &mut paid, cap);
    #[cfg(test)]
    PROTOCOL_WORK.update(|stats| {
        *stats = (
            prepaid - paid.remaining(),
            kernel.dependency_count(),
            result.as_ref().map_or(0, |r| r.steps),
        )
    });
    let scalar = rules.right.right.right.rule();
    // A rule may return a semantic error after catching a meter refusal. The
    // operation's sticky work/cancellation stop still takes precedence.
    let status = if let Err(error) = paid.checkpoint() {
        paid.failure_status(&KernelError::Stopped(error))
    } else {
        match result {
            Ok(ref report) if report.closure == Closure::Closed => ReductionStatus::Stable,
            Ok(_) => ReductionStatus::RoundLimit,
            Err(analysis_kernel::SolveError::Analyzer(
                Either::Right(Either::Right(Either::Right(ref error)))
                | Either::Right(Either::Right(Either::Left(ref error)))
                | Either::Right(Either::Left(ref error))
                | Either::Left(ref error),
            )) => match error {
                RuleError::Fact(error) => super::reduce::error_status(error),
                RuleError::Kernel(error) => paid.failure_status(error),
            },
            Err(analysis_kernel::SolveError::Kernel(ref error)) => paid.failure_status(error),
        }
    };
    NumericReduction {
        value: scalar.value.clone(),
        status,
        rounds: scalar.rounds,
        strengthened: scalar.strengthened,
    }
}
fn limited(value: NumericValue, status: ReductionStatus) -> NumericReduction<NumericValue> {
    NumericReduction {
        value,
        status,
        rounds: 0,
        strengthened: 0,
    }
}
