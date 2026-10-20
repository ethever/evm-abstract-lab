//! Explicit cost contracts for the selected fixed-input reduction assembly.

use super::{DomainSpec, bits::BoundsToMask, bounds::MaskToBounds};

/// Upper bound for one component evaluation, including its `Charges`, queries,
/// publications and publication policies. Dispatch and final reporting are paid
/// by the assembly. Quotes must be cheap, pure and saturate at `usize::MAX`.
///
/// `finite` bounds every retained/published finite cover in this invocation.
/// Implementations use `Charges`, `Read` and `Accepts` on the three existing
/// channels in the supplied scope. They must not reenter scheduling, add jobs,
/// scopes or external dependencies, or replace the meter.
/// A rule with different resource needs can select its own [`ProductWork`].
/// This is an explicit author-supplied bound, never an inference from rule code.
pub trait RuleWork {
    /// Bound one evaluation at any point in the descending closure.
    fn evaluation_work(&self, spec: DomainSpec, finite: usize) -> usize;
}

/// Caller-funded budget policy for exactly the selected mask/range rules.
///
/// The same quote is used in outer transfer/refinement payment and in the local
/// ledger. It covers setup, all rule/capability work and final kernel reporting;
/// it is not another root budget. `finite` is at least the configured capacity.
pub trait ProductWork<M, R>: std::fmt::Debug {
    /// Bound a whole closure, or return `usize::MAX` to refuse execution.
    fn closure_work(&self, spec: DomainSpec, masks: &M, bounds: &R, finite: usize) -> usize;
}

/// Compose the selected component bounds over the entire permitted schedule.
/// No convergence shortcut for a different pair of rules is assumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComposedWork;
impl<M: RuleWork, R: RuleWork> ProductWork<M, R> for ComposedWork {
    fn closure_work(&self, spec: DomainSpec, masks: &M, bounds: &R, finite: usize) -> usize {
        let Some(steps) = spec
            .reduction_rounds()
            .checked_mul(3)
            .filter(|steps| steps.checked_add(1).is_some())
        else {
            return usize::MAX;
        };
        let per_step = masks
            .evaluation_work(spec, finite)
            .max(bounds.evaluation_work(spec, finite))
            .max(scalar_work(spec, finite));
        // At most four queued jobs: selection, handle, begin and finish cost 7
        // after the seed. Any component may rerun, so reserve the largest bound
        // for every remaining step rather than assuming a built-in wake order.
        fixed_work(finite).saturating_add(per_step.saturating_add(7).saturating_mul(steps))
    }
}

/// Explicit fallback allowance for a caller that bounds the whole closure.
/// It may intentionally stop before closure; refusal reports `WorkLimit`.
/// It neither asserts convergence nor derives the cost of replacement rules.
#[derive(Clone, Copy, Debug)]
pub struct BoundedWork(pub usize);
impl<M, R> ProductWork<M, R> for BoundedWork {
    fn closure_work(&self, _: DomainSpec, _: &M, _: &R, _: usize) -> usize {
        self.0
    }
}

// These tariffs follow the fixed topology: one scope, three singleton channels,
// four jobs and at most twelve live read edges (three values plus the scope
// head per non-seed job). Component quotes include every capability call;
// repeated calls must be counted repeatedly by the component author.
pub(super) fn query_work(value: usize) -> usize {
    9usize.saturating_add(28).saturating_add(value)
}
pub(super) fn publish_work(policy: usize, value: usize) -> usize {
    9usize
        .saturating_add(policy)
        .saturating_add(value)
        .saturating_add(38)
}
fn fixed_work(finite: usize) -> usize {
    let setup: usize = 11 + 3 * 4 + 4 * (7 + 4 + 5) + 1;
    // Seed runs first: three new keys, no readers/policy meets, eight units of
    // dispatch and complete scalar payload retention.
    let seed = (3 * (6 + 8 + 2) + 24 + 8usize).saturating_add(finite);
    // solve_report's metadata scan plus its scope/job/fact/reader traversal.
    let report = 8 + 16 + 4 * 4 + 8 * 4 + 6 * 7 + 4 + 6 * 12;
    setup.saturating_add(seed).saturating_add(report)
}
fn scalar_work(spec: DomainSpec, finite: usize) -> usize {
    4096usize
        .saturating_add(256usize.saturating_mul(spec.capacity().max(finite)))
        .saturating_add(32usize.saturating_mul(spec.capacity().max(finite).saturating_add(32)))
        .saturating_add(2 * query_work(8))
        .saturating_add(query_work(finite.saturating_add(8)))
        .saturating_add(publish_work(16, 8))
        .saturating_add(publish_work(128, 8))
        .saturating_add(publish_work(
            finite
                .saturating_mul(2)
                .saturating_add(16)
                .saturating_mul(64),
            finite.saturating_add(8),
        ))
}

impl RuleWork for BoundsToMask {
    fn evaluation_work(&self, _: DomainSpec, _: usize) -> usize {
        256 + query_work(8) + publish_work(16, 8)
    }
}
impl RuleWork for MaskToBounds {
    fn evaluation_work(&self, _: DomainSpec, finite: usize) -> usize {
        4096usize
            .saturating_add(2 * query_work(8))
            .saturating_add(query_work(finite.saturating_add(8)))
            .saturating_add(publish_work(128, 8))
    }
}
impl RuleWork for super::KeepMasks {
    fn evaluation_work(&self, _: DomainSpec, _: usize) -> usize {
        0
    }
}
impl RuleWork for super::KeepBounds {
    fn evaluation_work(&self, _: DomainSpec, _: usize) -> usize {
        0
    }
}
