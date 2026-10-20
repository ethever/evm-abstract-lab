//! Immutable context and optional precision partitions for one execution run.
//!
//! Mandatory machine shape is independent of these policies. A policy can
//! combine optional contexts, but cannot combine differently shaped stacks,
//! executable modes, storage owners or continuation kinds. Policies stay fixed
//! while inputs, summaries and certificates from the run are retained.

use std::collections::BTreeSet;

/// Intraprocedural context selection, shared by ordinary and relative execution.
///
/// `advance` must retain at most `depth` elements. `normalize` must be
/// idempotent, must not increase length, and must retain only context metadata;
/// neither operation changes executable values. Callers reserve `work` before advance and `normalization_work` before
/// normalize and check cancellation around the operation.
pub trait ContextPolicy {
    /// Append one jump source under the configured finite context bound.
    fn advance(&self, history: &mut Vec<usize>, jump_source: usize, depth: usize);

    /// Canonicalize an already bounded context for a relative procedure input.
    fn normalize(&self, history: &mut Vec<usize>);

    /// Conservative logical charge for advancing a context.
    fn work(&self, length: usize) -> usize;

    /// Charge for canonicalizing retained history. The default shares `work`;
    /// policies with a more expensive normalization must override this quote.
    fn normalization_work(&self, length: usize) -> usize {
        self.work(length)
    }
}

/// The most recent occurrence of each jump source, in execution order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecentDistinctJumps;

impl ContextPolicy for RecentDistinctJumps {
    fn advance(&self, history: &mut Vec<usize>, jump_source: usize, depth: usize) {
        history.retain(|source| *source != jump_source);
        history.push(jump_source);
        let discard = history.len().saturating_sub(depth);
        history.drain(..discard);
    }

    fn normalize(&self, history: &mut Vec<usize>) {
        let mut seen = BTreeSet::new();
        history.reverse();
        history.retain(|source| seen.insert(*source));
        history.reverse();
    }

    fn work(&self, length: usize) -> usize {
        length.saturating_mul(2).saturating_add(1)
    }

    fn normalization_work(&self, length: usize) -> usize {
        let levels = usize::try_from(length.max(1).ilog2())
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        length.saturating_mul(levels).saturating_mul(8)
    }
}

/// Combine all intraprocedural jump contexts at each mandatory machine shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextInsensitive;

impl ContextPolicy for ContextInsensitive {
    fn advance(&self, history: &mut Vec<usize>, _: usize, _: usize) {
        history.clear();
    }

    fn normalize(&self, history: &mut Vec<usize>) {
        history.clear();
    }

    fn work(&self, length: usize) -> usize {
        length.saturating_add(1)
    }
}

/// Optional precision information associated with one incoming transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arrival {
    /// A normal CALL result for precisely its first resumed basic block.
    /// Intraprocedural successors and CREATE results carry no status partition.
    pub resume_status: Option<bool>,
}

/// A finite optional partition, applied after mandatory shape selection.
///
/// Classification is deterministic for a frozen policy and arrival. New
/// classes consume the same state/work capacities as new structural nodes;
/// a classification is never evidence that an input is unreachable.
pub trait PartitionPolicy {
    /// Internal precision identity; it does not replace public machine shape.
    type Class: Clone + Ord;

    /// Select the precision class for an incoming state.
    fn classify(&self, arrival: Arrival) -> Self::Class;
}

/// Keep immediate CALL success and failure apart for one resumed block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FirstResumeStatus;

impl PartitionPolicy for FirstResumeStatus {
    type Class = Option<bool>;

    fn classify(&self, arrival: Arrival) -> Self::Class {
        arrival.resume_status
    }
}

/// Combine immediate CALL results at the same mandatory machine shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeResumeStatus;

impl PartitionPolicy for MergeResumeStatus {
    type Class = ();

    fn classify(&self, _: Arrival) -> Self::Class {}
}

/// Frozen, statically selected precision policies for one EVM execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnginePolicies<C = RecentDistinctJumps, P = FirstResumeStatus> {
    /// Intraprocedural context policy used by every transfer path.
    pub context: C,
    /// Optional precision partition used by ordinary and relative admission.
    pub partition: P,
}

impl<C, P> EnginePolicies<C, P> {
    /// Compose policies without changing instruction dispatch or the solver.
    pub const fn new(context: C, partition: P) -> Self {
        Self { context, partition }
    }
}

impl Default for EnginePolicies {
    fn default() -> Self {
        Self::new(RecentDistinctJumps, FirstResumeStatus)
    }
}
