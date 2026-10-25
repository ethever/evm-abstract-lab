//! Immutable numerical closure evidence, independent of executable roots.
//!
//! A model handle never supplies procedure inputs. Public stores have already
//! passed the owner/alias projection; rich replay is a separate paid operation.

use super::{EvmByteArray, EvmStore, FrameKey, FrontierReason, OutcomeKind};
use crate::{
    analysis::{EvmProcessInvocation, TransferSemantics, transfer::procedure::EvmRegionalCfg},
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement},
    resource::WorkBudget,
    world::{EvmByteArraySummary, EvmStoreSummary},
};
use alloy_primitives::B256;
use serde::{Serialize, Serializer};
use std::{ops::Deref, sync::Arc};

mod materialize;

/// Table snapshot identity within one analysis and relative namespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ProcedureEvidenceVersion {
    /// Relative table namespace; source World IDs and graph namespaces stay separate.
    pub namespace: usize,
    /// Zero identifies a refused/unavailable model, never a certified snapshot.
    pub generation: usize,
}

/// Actual native producer versions, independent of a report or World source ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProcedureModelVersion {
    /// Relative table namespace; source World IDs and graph namespaces stay separate.
    pub namespace: usize,
    /// Actual covering entry revision published by the native procedure owner.
    pub entry_revision: usize,
    /// Actual kernel output-slot version, including semantic scheduling publications.
    pub output_revision: u64,
    /// Frozen physical owner/source vocabulary, when Process preparation applies.
    pub source_epoch: Option<B256>,
}

/// Numerical output; no symbolic replay, write history or caller continuation.
#[derive(Debug, Serialize)]
pub struct EvmProcedureModelOutput<N: WordElement> {
    /// Relative termination category; each caller owns its rollback checkpoint.
    pub kind: OutcomeKind,
    /// Native generation of this termination alternative.
    pub generation: usize,
    /// Canonical return/revert byte and length bounds.
    pub data: EvmByteArraySummary<N>,
    /// Physical transaction-effect bounds, after namespace/alias projection.
    pub store: EvmStoreSummary<N>,
}

/// Funded minimum used by semantic closure and metadata/control projection.
#[derive(Debug, Serialize)]
pub struct EvmProcedureModelData<N: WordElement> {
    /// Producer identities on which this immutable numerical row depends.
    pub version: ProcedureModelVersion,
    /// Joined logical ADDRESS/CALLER/CALLVALUE bounds; absent for Native frames.
    pub process_input: Option<EvmProcessInvocation<N>>,
    /// Interpretation independent of the captured source identity.
    pub semantics: TransferSemantics,
    /// Captured source, owner, static mode and finite procedure context.
    pub frame: FrameKey,
    /// Physical whole-store entry invariant, including lifecycle and transient state.
    pub input: EvmStoreSummary<N>,
    /// Joint data/store alternatives indexed by relative termination category.
    pub outputs: Vec<EvmProcedureModelOutput<N>>,
    /// Actual callee procedure IDs in this immutable directory.
    pub dependencies: Vec<usize>,
    /// Number of strict covering entry expansions.
    pub input_updates: usize,
    /// Number of native output relation publications.
    pub output_updates: usize,
    /// Historical visited blocks; current execution requires regional receipts.
    pub covered_blocks: Vec<usize>,
}

/// Shared immutable semantic row. Clone copies one handle, never native values.
#[derive(Debug)]
pub struct EvmProcedureModel<N: WordElement>(Arc<EvmProcedureModelData<N>>);
impl<N: WordElement> EvmProcedureModel<N> {
    pub(in crate::analysis) fn new(row: EvmProcedureModelData<N>) -> Self {
        Self(Arc::new(row))
    }
    /// Whether both handles retain the same allocation; this does not certify currentness.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<N: WordElement> Clone for EvmProcedureModel<N> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<N: WordElement> Deref for EvmProcedureModel<N> {
    type Target = EvmProcedureModelData<N>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<N: WordElement> Serialize for EvmProcedureModel<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

/// A table snapshot shares its unchanged rows, and unchanged snapshots share
/// this entire directory. Refusal uses an allocation-free empty marker.
/// Neither handle grants a mutable view.
#[derive(Debug)]
pub struct EvmProcedureModels<N: WordElement>(Option<Arc<Vec<EvmProcedureModel<N>>>>);
impl<N: WordElement> EvmProcedureModels<N> {
    pub(in crate::analysis) fn new(rows: Vec<EvmProcedureModel<N>>) -> Self {
        Self(Some(Arc::new(rows)))
    }
    /// Same retained allocation (or both empty); this does not certify currentness.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}
impl<N: WordElement> Default for EvmProcedureModels<N> {
    fn default() -> Self {
        Self(None)
    }
}
impl<N: WordElement> Clone for EvmProcedureModels<N> {
    fn clone(&self) -> Self {
        Self(self.0.as_ref().map(Arc::clone))
    }
}
impl<N: WordElement> Deref for EvmProcedureModels<N> {
    type Target = [EvmProcedureModel<N>];
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}
impl<'a, N: WordElement> IntoIterator for &'a EvmProcedureModels<N> {
    type Item = &'a EvmProcedureModel<N>;
    type IntoIter = std::slice::Iter<'a, EvmProcedureModel<N>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<N: WordElement> Serialize for EvmProcedureModels<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.deref().serialize(serializer)
    }
}

/// Regional receipts are immutable and shared independently of numeric rows.
#[derive(Debug, Default)]
pub struct EvmRegionalEvidence(Option<Arc<EvmRegionalCfg>>);
impl EvmRegionalEvidence {
    pub(in crate::analysis) fn new(cfg: EvmRegionalCfg) -> Self {
        Self(Some(Arc::new(cfg)))
    }
    /// Same retained allocation (or both empty); this does not certify currentness.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}
impl Clone for EvmRegionalEvidence {
    fn clone(&self) -> Self {
        Self(self.0.as_ref().map(Arc::clone))
    }
}
impl Deref for EvmRegionalEvidence {
    type Target = EvmRegionalCfg;
    fn deref(&self) -> &Self::Target {
        static EMPTY: EvmRegionalCfg = EvmRegionalCfg {
            nodes: Vec::new(),
            calls: Vec::new(),
        };
        self.0.as_deref().unwrap_or(&EMPTY)
    }
}
impl Serialize for EvmRegionalEvidence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.deref().serialize(serializer)
    }
}

/// Optional rich expansion has no authority over semantic closure/currentness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ProcedureReportOutcome {
    /// No rich replay has been requested for this snapshot.
    NotRequested,
    /// Every requested row was expanded and paid.
    Complete,
    /// Expansion was refused; semantic closure and currentness are unchanged.
    Refused(FrontierReason),
}

/// Result of an optional rich replay, with its own explicit cost and outcome.
#[derive(Debug, Serialize)]
pub struct EvmProcedureReport<N: WordElement> {
    /// Immutable semantic snapshot from which these rows were expanded.
    pub version: ProcedureEvidenceVersion,
    /// Optional expansion outcome, independent of execution/closure status.
    pub outcome: ProcedureReportOutcome,
    /// All expanded rows on success; empty after refusal or no request.
    pub procedures: Vec<EvmProcedureSummary<N>>,
    /// Actual successful charges added to the supplied persistent allowance.
    pub postprocess_work: usize,
}

impl<N: WordElement> EvmProcedureReport<N> {
    /// Initial optional-report state, without requesting replay or spending work.
    pub fn not_requested(version: ProcedureEvidenceVersion) -> Self {
        Self {
            version,
            outcome: ProcedureReportOutcome::NotRequested,
            procedures: Vec::new(),
            postprocess_work: 0,
        }
    }
}

/// One numerical output of a caller-independent procedure relation.
/// Optional presentation replay owns independent symbolic roots.
#[derive(Clone, Debug, Serialize)]
pub struct EvmProcedureSummaryOutput<N: WordElement> {
    /// Relative frame termination; failure effects are rolled back at each caller.
    pub kind: OutcomeKind,
    /// Bound on full return or revert data.
    pub data: EvmByteArray<N>,
    /// Bound on transaction effects at completion.
    pub store: EvmStore<N>,
}

/// A procedure participating in the joint known/unknown fixed point.
#[derive(Clone, Debug, Serialize)]
pub struct EvmProcedureSummary<N: WordElement> {
    /// Joined logical invocation bounds for a process; None for native code.
    pub process_input: Option<EvmProcessInvocation<N>>,
    /// Interpretation is independent of the captured Program/code identity.
    pub semantics: TransferSemantics,
    /// Executable and context shape, independent of suspended caller stacks.
    pub frame: FrameKey,
    /// Whole-store numerical input invariant, including transient and lifecycle state.
    pub input: EvmStore<N>,
    /// Growing output alternatives by termination category.
    pub outputs: Vec<EvmProcedureSummaryOutput<N>>,
    /// Procedures whose output growth replays retained return continuations.
    pub dependencies: Vec<usize>,
    /// Number of strict input expansions.
    pub input_updates: usize,
    /// Number of strict output expansions.
    pub output_updates: usize,
    /// Cumulative historical observations of visited captured-program blocks.
    /// These do not certify execution of the current joined input; use current
    /// regional transfer receipts for that distinction. Unknown PCs are never invented.
    pub covered_blocks: Vec<usize>,
}

/// Why ordinary stack execution entered the relative procedure worklist.
/// The trigger is captured before callee detachment; recursive dependencies
/// discovered during saturation do not change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ProcedureSummaryKind {
    /// Captured runtime selected by the explicit coarse process policy.
    KnownProcess,
    /// An unobserved implementation or symbolic target remainder.
    UnknownEnvironment,
    /// A known runtime CALL entry admitted before any repeated active call edge.
    KnownCall,
    /// A known runtime entry selected by a repeated structural call edge on its stack.
    KnownRecursion,
}

/// Coverage evidence for an entry and its recursive procedure system.
#[derive(Clone, Debug, Serialize)]
pub struct EvmProcedureSummaryRecord<N: WordElement> {
    /// Identity of this immutable semantic snapshot; distinct from World revision.
    pub version: ProcedureEvidenceVersion,
    /// Main machine node whose execution requested this relation.
    pub source_state: usize,
    /// Certified for this source's final successful, still-valid World evaluation.
    /// False means uncertified currentness, not proof that historical facts are
    /// stale or false. Regional closure and global closure remain separate.
    pub source_current: bool,
    /// Actual source of the abstraction; known runtime remains known runtime.
    pub kind: ProcedureSummaryKind,
    /// Entry procedure index, absent when entry or evidence construction failed.
    pub root_procedure: Option<usize>,
    /// Regional-system closure observed at this snapshot's retained input and
    /// dependency basis; neither current global closure nor source currency.
    pub closed: bool,
    /// Known bytecode and unknown procedures solved together.
    pub procedures: EvmProcedureModels<N>,
    /// Coarse native regional CFG, separate from machine instruction/SSA evidence.
    pub regional_cfg: EvmRegionalEvidence,
    /// Relative block states retained by the shared worklist.
    pub state_count: usize,
    /// Metered block/environment transfers performed for this request.
    pub transfers: usize,
    /// Output-to-caller propagations, including local failures that resume the environment.
    pub propagations: usize,
    /// Existing procedure relations used by a retained caller continuation.
    pub reused: usize,
}
