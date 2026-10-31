//! Explicit cost contracts for the selected fixed-input reduction assembly.

use super::{DomainSpec, bits::BoundsToMask, bounds::MaskToBounds};
use analysis_kernel::cost::{self as kernel, ReportCounts};

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
/// it is not another root budget. Generic work uses `finite` at least as large
/// as the configured capacity; exact built-in schedules may prove a smaller cover.
pub trait ProductWork<M, R>: std::fmt::Debug {
    /// Bound a whole closure, or return `usize::MAX` to refuse execution.
    fn closure_work(&self, spec: DomainSpec, masks: &M, bounds: &R, finite: usize) -> usize;
}

/// Work proof for exactly BoundsToMask + MaskToBounds + the built-in scalar
/// rule. Replacement rules cannot attach this policy to a different assembly.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinWork;
impl ProductWork<BoundsToMask, MaskToBounds> for BuiltinWork {
    fn closure_work(
        &self,
        spec: DomainSpec,
        _: &BoundsToMask,
        _: &MaskToBounds,
        finite: usize,
    ) -> usize {
        spec.numeric_closure_work(finite, None)
    }
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

// Shared by generic assembly and the exact built-in scheduling proof.
// One unselected scope, three singleton channels and four jobs give ten facts:
// a scope head, three global directories, three scoped directories and values.
// A non-seed job has at most four read/required edges, and any value/head has
// at most three readers/demands. There are no producer guards, foreign premises,
// lifecycle operations or explicit obligations in this ephemeral topology.
// Every capability call is counted, including repeat observations of one edge.
const INTERN_WORK: usize = kernel::tree_work(1) + kernel::key_lookup_work(1, 1);
pub(in crate::domain) const STEP_WORK: usize = kernel::dispatch_work(3);

pub(in crate::domain) fn query_work(value: usize) -> usize {
    // Required observes both the scope head and value. The numeric topology has
    // at most four read/required edges per job and three readers/demands per fact.
    let observation = kernel::observation_work(4, 3).saturating_add(kernel::requirement_work(4, 3));
    INTERN_WORK
        .saturating_add(observation.saturating_mul(2))
        .saturating_add(value)
}
pub(in crate::domain) fn publish_work(policy: usize, value: usize) -> usize {
    // A descending replacement has one changed fact and no semantic seeds.
    // At most three pending jobs and three wakes; use a full queue to cover all
    // geometric growth, even when some of those readers are already pending.
    // Numeric capabilities use value_work for both clone and retained-old drop.
    INTERN_WORK
        .saturating_add(policy)
        .saturating_add(kernel::change_plan_work(1))
        .saturating_add(kernel::change_install_work(
            1,
            3,
            kernel::enqueue_work(3, 3, 3),
            kernel::publication_value_work(value, value),
        ))
}

fn first_write_work(scope_facts: usize) -> usize {
    // This channel has no scope directory or key yet. Creating the directory
    // adds one fact before interning its singleton unit key.
    kernel::tree_work(0)
        .saturating_add(kernel::channel_scope_insert_work(0, scope_facts))
        .saturating_add(kernel::key_lookup_work(0, 1))
        .saturating_add(kernel::key_insert_work(0, 1, scope_facts.saturating_add(1)))
}

pub(in crate::domain) fn fixed_work(finite: usize) -> usize {
    // Successive queue lengths/minimum capacities are 0/0, 1/1, 2/2 and 3/4.
    let registration = kernel::registration_work(0, kernel::enqueue_work(0, 0, 1))
        .saturating_add(kernel::registration_work(1, kernel::enqueue_work(1, 1, 1)))
        .saturating_add(kernel::registration_work(2, kernel::enqueue_work(2, 2, 1)))
        .saturating_add(kernel::registration_work(3, kernel::enqueue_work(3, 4, 1)));
    let setup = kernel::tree_work(0)
        .saturating_add(kernel::scope_insert_work(0))
        .saturating_add(kernel::CHANNEL_WORK.saturating_mul(3))
        .saturating_add(registration)
        .saturating_add(kernel::SEAL_SCOPE_WORK);
    // Three first writes each add a directory and value to this scope. Each
    // changes two facts and classifies only its directory as a semantic seed.
    // No old values/readers; each guarantee has eight fields, plus finite data.
    let seed_publication = kernel::semantic_seed_work(0, 1)
        .saturating_add(kernel::change_plan_work(2))
        .saturating_add(kernel::change_install_work(
            2,
            0,
            kernel::enqueue_work(3, 4, 0),
            kernel::publication_value_work(8, 0),
        ));
    let seed = first_write_work(1)
        .saturating_add(first_write_work(3))
        .saturating_add(first_write_work(5))
        .saturating_add(seed_publication.saturating_mul(3))
        .saturating_add(finite)
        .saturating_add(kernel::dispatch_work(4));
    // solve reports <=3 pending jobs, all four jobs/ten facts/one scope, at most
    // twelve read edges and no producer-family coverage in this assembly.
    let report =
        kernel::report_metadata_work(10).saturating_add(kernel::report_work(ReportCounts {
            pending: 3,
            jobs: 4,
            facts: 10,
            scopes: 1,
            readers: 12,
            coverage: 0,
        }));
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
