//! Frozen context selection and composable complete-state semantic policies.
//!
//! Partition preparation establishes the covering representation on which its
//! own key is valid. Policies stay fixed while inputs and summaries are retained.

use std::collections::BTreeSet;

use crate::analysis::{MachineEdgeKind, OutcomeKind};

mod key;
mod ownership;
mod semantic;

pub(in crate::analysis) use key::{KeyIndex, checkpoint as key_checkpoint, classify, clone_key};
pub use key::{PolicyKey, QuotedKey};
pub use ownership::{PartitionRefusal, PartitionResult, RelativePartitionResult};

/// The producer's typed CALL outcome already proves its boolean result. Users
/// of this projection must establish that the edge resumes CALL, not CREATE.
pub(in crate::analysis) fn call_result(kind: MachineEdgeKind) -> Option<bool> {
    match kind {
        MachineEdgeKind::Return | MachineEdgeKind::Summary(OutcomeKind::Return) => Some(true),
        MachineEdgeKind::Revert
        | MachineEdgeKind::Failure
        | MachineEdgeKind::Summary(OutcomeKind::Revert | OutcomeKind::Failure) => Some(false),
        _ => None,
    }
}

pub use semantic::{
    Acceleration, AccelerationInput, AccelerationPolicy, BoundedJoin, CoarseSummaries,
    CorrelatedJoin, EagerAcceleration, FeedbackAcceleration, ForgetJumpHistory, MachinePartition,
    NoAcceleration, Partitioned, PartitionedRelative, PolicyContext, PolicySite, ProcedureOutput,
    ProcedureSummaryPolicy, ProjectedSummaries, RelativeState, SemanticPolicies, StatePolicy,
    StructuralPartition, WholeStateMetadata, WholeStatePolicy,
};

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
    type Class: PolicyKey;

    /// Select the precision class for an incoming state.
    fn classify(&self, arrival: Arrival) -> Self::Class;

    /// Conservative quote for classification, without executing it. The engine
    /// reserves this work before calling `classify` and checks cancellation
    /// around both operations. Opaque legacy classifiers refuse by default.
    fn classification_work(&self, _: Arrival) -> usize {
        usize::MAX
    }
}

/// Keep immediate CALL success and failure apart for one resumed block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FirstResumeStatus;

impl PartitionPolicy for FirstResumeStatus {
    type Class = Option<bool>;

    fn classify(&self, arrival: Arrival) -> Self::Class {
        arrival.resume_status
    }

    fn classification_work(&self, _: Arrival) -> usize {
        1
    }
}

/// Supply one arrival class, leaving complete identity to the semantic policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeResumeStatus;

impl PartitionPolicy for MergeResumeStatus {
    type Class = ();

    fn classify(&self, _: Arrival) -> Self::Class {}

    fn classification_work(&self, _: Arrival) -> usize {
        1
    }
}

/// Frozen, statically selected precision policies for one EVM execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnginePolicies<C = RecentDistinctJumps, P = FirstResumeStatus, S = SemanticPolicies> {
    /// Intraprocedural context policy used by every transfer path.
    pub context: C,
    /// Optional precision partition used by ordinary and relative admission.
    pub partition: P,
    /// Covering preparation, whole-state admission, acceleration and summaries.
    pub state: S,
}

impl<C, P> EnginePolicies<C, P> {
    /// Compose policies without changing instruction dispatch or the solver.
    pub const fn new(context: C, partition: P) -> Self {
        Self {
            context,
            partition,
            state: SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        }
    }
}

impl<C, P, S> EnginePolicies<C, P, S> {
    /// Replace the complete-state assembly without requiring clonable policies.
    pub fn with_state<T>(self, state: T) -> EnginePolicies<C, P, T> {
        EnginePolicies {
            context: self.context,
            partition: self.partition,
            state,
        }
    }
}

impl Default for EnginePolicies {
    fn default() -> Self {
        Self::new(RecentDistinctJumps, FirstResumeStatus)
    }
}
