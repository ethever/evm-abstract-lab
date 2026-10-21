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
        // The seed has no reads and never reruns. Any of the three remaining
        // jobs may rerun, so reserve the largest bound for every remaining step
        // rather than assuming the built-in rules' wake order.
        fixed_work(finite).saturating_add(per_step.saturating_add(STEP_WORK).saturating_mul(steps))
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

// Shared by the generic assembly and Domain's separately proved schedules.
// One unselected scope, three singleton channels and four jobs give ten facts:
// a scope head, three global directories, three scoped directories and values.
// A non-seed job has at most four read/required edges, and any value/head has
// at most three readers/demands. There are no producer guards, foreign premises,
// lifecycle operations or explicit obligations in this ephemeral topology.
// Kernel tree_work(n) = floor(log2(n + 1)) + 2, so t(1)=3 and t(3..=6)=4.
// Every capability call is counted, including repeat observations of one edge.
const INTERN_WORK: usize = 3 + 9; // existing scope table and existing unit key
pub(in crate::domain) const STEP_WORK: usize = 3 + (2 * 4 + 12) + 3;

pub(in crate::domain) fn query_work(value: usize) -> usize {
    // Required observe: 2*t(reads) + t(readers) + 2, then t(required) +
    // t(demands) + 2 = 24. Both the scope head and value are observed. Discovery
    // pays only the first 14; numeric Read uses Required, so reserve both indexes.
    INTERN_WORK.saturating_add(2 * 24).saturating_add(value)
}
pub(in crate::domain) fn publish_work(policy: usize, value: usize) -> usize {
    // A descending replacement has one changed fact and no semantic seeds.
    // Plan7 + install1 + fact2 + batch enqueue66 + three wake visits9 = 85.
    // The batch upper bound includes geometric growth: with <=3 pending jobs
    // and <=3 wakes, 3*(t(6)+6) + 6*6 = 66, even for duplicate pending readers.
    // Numeric capabilities use value_work for both clone and retained-old drop.
    INTERN_WORK
        .saturating_add(policy)
        .saturating_add(value.saturating_mul(2))
        .saturating_add(85)
}
pub(in crate::domain) fn fixed_work(finite: usize) -> usize {
    // Scope11, channels12, seal1. Registration pays 7 + t(jobs) + enqueue;
    // successive queue lengths/capacities are 0/0, 1/1, 2/2 and 3/4.
    let registration = (7 + 2 + 15) + (7 + 3 + 21) + (7 + 3 + 28) + (7 + 4 + 10);
    let setup: usize = 11 + 3 * 4 + registration + 1;
    // Each first write interns its scoped directory and value: 29,31,31.
    // Each changes two facts: semantic-directory classification4, plan13,
    // fact writes4 and installation1, plus its value. No old value/readers yet.
    // Seed dispatch has four queued jobs: selection24 + handle/begin/finish3.
    let seed = (29 + 31 + 31 + 3 * (4 + 13 + 4 + 1) + 24 + 27usize).saturating_add(finite);
    // solve calls solve_report, not closure_in_metered: metadata11, base16,
    // <=3 pending, four jobs, ten facts, one scope and <=12 read edges. Required
    // indexes are covered by the new 12-per-fact traversal; coverage stays zero.
    let report = 11 + 16 + 4 * 3 + 8 * 4 + 12 * 10 + 4 + 6 * 12;
    setup.saturating_add(seed).saturating_add(report)
}
pub(in crate::domain) fn scalar_work(spec: DomainSpec, finite: usize) -> usize {
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
