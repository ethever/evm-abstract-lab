//! A fixed-snapshot upper bound survives optional, source-qualified discovery.

mod acquire;
mod run;

pub use run::analyze_rpc_hybrid;

use super::{RpcAcquisition, RpcAnalysisError};
use crate::{
    analysis::{
        EvmWorldAnalysis, HybridAnalysis, HybridLocalDetail, Limit, Status,
        layers::{FactRevision, SessionError, SnapshotId},
    },
    domain::WordElement,
    world::rpc::RpcError,
};
use serde::Serialize;

/// Relation between the original certified input and currently installed facts.
/// This does not assert equality of graphs or refresh any source-current guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RpcBaselineRelation {
    /// The original baseline did not obtain its own closure certificate.
    Uncertified,
    /// No initial facts changed after the original baseline.
    SameRevision,
    /// Accepted exact facts restrict the same snapshot/fork and unchanged
    /// original entry/root binding/base:
    /// concretization(current facts) is a subset of concretization(original).
    /// The ORIGINAL closed baseline therefore covers that subset. Its code
    /// sources and currentness remain qualified by the ORIGINAL revision only.
    InitialFactRestriction,
}

/// One atomically accepted restriction, produced by the existing Session bridge.
/// Catalog extension alone cannot mint this receipt; fact publication must succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RpcFactRestriction {
    /// Initial-state set before this batch; previous observations remain intact.
    pub previous: FactRevision,
    /// New initial-state subset at the identical snapshot.
    pub current: FactRevision,
    /// Number of additional exact observations, including known zero values.
    pub added_observations: usize,
}

/// Why optional precision ended, independently of global upper-bound coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RpcPrecisionOutcome {
    /// The caller requested no optional precision.
    Disabled,
    /// The original baseline did not certify global closure.
    BaselinePending,
    /// Native detail is closed at the currently installed fact revision UNDER
    /// its reported HybridNativeBinding and effective Entry.binding. A namespace
    /// condition does not certify the original unconditional input or replace
    /// the conservative baseline.
    Current,
    /// Actual native or process obligations remain; inspect the retained report.
    Incomplete,
    /// Original or lowered optional work ceiling refused the next operation.
    Work,
    /// Cumulative state/transfer limits prevented further acquisition/execution.
    Budget(Limit),
    /// A real fixed-snapshot RPC acquisition failed; original errors are retained.
    AcquisitionFailed,
    /// Paid fact projection/refinement or analyzer assembly rejected the operation.
    PreparationFailed,
}

/// Global coverage and current precision have distinct source/revision bindings.
#[derive(Clone, Debug, Serialize)]
pub struct RpcHybridCoverage {
    /// Snapshot shared by the original baseline, all restrictions and all restarts.
    pub snapshot: SnapshotId,
    /// Exact revision of the immutable original baseline and its source guards.
    pub baseline_revision: FactRevision,
    /// Last atomically installed fact revision, even if its restart was refused.
    pub current_revision: FactRevision,
    /// Revision of the last retained native graph, which can precede current facts.
    pub detail_revision: Option<FactRevision>,
    /// Whether the retained detail consumed current facts. This checks fact
    /// revision equality only, not equality of original/effective root bindings.
    /// The detail report's condition, Entry.binding and source guards still apply.
    pub detail_at_current_revision: bool,
    /// Initial-set coverage qualification; never a relabelled new graph proof.
    pub baseline_relation: RpcBaselineRelation,
    /// Ordered, validated fact restrictions since the original baseline.
    pub restrictions: Vec<RpcFactRestriction>,
    /// Optional precision's independent completion/failure reason.
    pub precision: RpcPrecisionOutcome,
    /// Real RPC acquisition and cumulative state allocation evidence.
    pub acquisition: RpcAcquisition,
    /// Total accepted work on the one persistent Session ledger.
    pub work: usize,
    /// Accepted optional work, including acquisition, projection and every restart.
    pub precision_work: usize,
    /// Original declared optional cap; no per-round reset is allowed.
    pub max_precision_work: usize,
    /// Cumulative transfers, including graphs discarded on fact refinement.
    pub transfers: usize,
}

/// Immutable original upper bound plus the latest independently qualified detail.
/// Neither acquisition failures nor partial optional execution rewrite the baseline.
#[derive(Debug)]
pub struct RpcHybridAnalysis<N: WordElement, E = std::convert::Infallible> {
    baseline: HybridAnalysis<N>,
    latest: Option<HybridAnalysis<N>>,
    retained_detail: Option<HybridAnalysis<N>>,
    coverage: RpcHybridCoverage,
    failures: Vec<RpcError>,
    preparation_failure: Option<SessionError<E>>,
}

pub(super) type RpcHybridResult<N, E> = Result<RpcHybridAnalysis<N, E>, RpcAnalysisError<E>>;
type RpcHybridParts<N, E> = (
    HybridAnalysis<N>,
    Option<HybridAnalysis<N>>,
    RpcHybridCoverage,
    Vec<RpcError>,
    Option<SessionError<E>>,
    Option<HybridAnalysis<N>>,
);

impl<N: WordElement, E> RpcHybridAnalysis<N, E> {
    /// Original report, certified only at its own snapshot/revision/source epoch.
    pub fn baseline(&self) -> &HybridAnalysis<N> {
        &self.baseline
    }

    /// Latest retained attempt; inspect its revision before treating detail as current.
    /// If a later projection failed, this deliberately remains the preceding report.
    pub fn latest(&self) -> &HybridAnalysis<N> {
        self.latest.as_ref().unwrap_or(&self.baseline)
    }

    /// Last useful native report, kept by move if a later attempt has no detail.
    /// Its binding may precede current facts; `latest()` still shows that attempt.
    pub fn detail_report(&self) -> Option<&HybridAnalysis<N>> {
        self.latest
            .as_ref()
            .filter(|report| report.detail().is_some())
            .or(self.retained_detail.as_ref())
    }

    /// Last useful native graph, including original frontiers and revision.
    pub fn detail(&self) -> Option<&EvmWorldAnalysis<N>> {
        self.detail_report().and_then(HybridAnalysis::detail)
    }

    /// Local native receipts keep their exact retained-input revision, additional
    /// root condition, source and phases. Native15 original/effective Entry.binding
    /// stays available even when no state or local receipt was admitted.
    /// Receipts from an older revision are historical observations of that input,
    /// even when their original per-entry currentness guard still holds.
    pub fn local_detail(&self) -> impl Iterator<Item = HybridLocalDetail<'_, N>> {
        self.detail_report()
            .into_iter()
            .flat_map(HybridAnalysis::local_detail)
    }

    /// Separate coverage, acquisition, revision and cumulative-resource evidence.
    pub fn coverage(&self) -> &RpcHybridCoverage {
        &self.coverage
    }

    /// Original transport/protocol failures, including optional cancelled requests.
    pub fn failures(&self) -> &[RpcError] {
        &self.failures
    }

    /// Actual optional projection/refinement/analyzer failure, without string erasure.
    pub fn preparation_failure(&self) -> Option<&SessionError<E>> {
        self.preparation_failure.as_ref()
    }

    /// Global closed upper-bound coverage of the currently restricted initial set.
    /// This is independent of optional detail completion.
    pub fn status(&self) -> Status {
        if self.coverage.baseline_relation == RpcBaselineRelation::Uncertified {
            Status::Incomplete
        } else {
            Status::Converged
        }
    }

    /// The original conservative graph remains the global relation. Precision
    /// consumers layer source-qualified local detail without deleting its routes.
    pub fn chosen(&self) -> &EvmWorldAnalysis<N> {
        self.baseline.baseline()
    }

    /// Cumulative accepted work, including discarded rounds and acquisition.
    pub fn work(&self) -> usize {
        self.coverage.work
    }

    /// Move reports and typed outcomes to an application without graph cloning.
    pub fn into_parts(self) -> RpcHybridParts<N, E> {
        (
            self.baseline,
            self.latest,
            self.coverage,
            self.failures,
            self.preparation_failure,
            self.retained_detail,
        )
    }
}

#[cfg(test)]
mod tests;
