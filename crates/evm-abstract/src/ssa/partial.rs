//! Names for recorded machine prefixes without claiming closed execution coverage.
//!
//! Only current execution receipts support instructions and transitions. Stale
//! prefixes and unsupported historical edges remain explicit gaps. Instruction
//! phases distinguish observed partial effects from completed opcode semantics;
//! unknown calls never acquire a fabricated result or return transition.

use super::{EffectId, EffectPhi, FramePhi, Instruction, SsaError, Transition, ValueId};
use crate::analysis::{
    InstructionProgress, MachineEdgeKind, MachineFrontier, Status, WorldAnalysis,
};
use serde::Serialize;

mod build;
#[cfg(test)]
mod tests;
mod verify;

/// Whether the retained transfer still describes this state's current entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PartialBlockCoverage {
    /// No transfer has supplied instruction or successor evidence.
    Unexecuted,
    /// The entry changed after the retained transfer; its body is not emitted.
    Stale,
    /// A current transfer supplied the displayed phases, possibly stopping early.
    Current,
}

/// An instruction attempt, with completed and pending effects kept distinct.
#[derive(Clone, Debug, Serialize)]
pub struct PartialInstruction {
    /// Bytecode and named operands; a pending step has no normal result.
    #[serde(flatten)]
    pub instruction: Instruction,
    /// The observed execution phase, independent of whole-graph completion.
    pub progress: InstructionProgress,
    /// Bundle before this instruction attempt.
    pub effect_input: EffectId,
    /// Observed bundle after the step, absent when it has only started.
    /// For `OperandsConsumed`, this names partial effects, not completed semantics.
    pub effect_result: Option<EffectId>,
}

/// A current prefix or a named entry waiting for a supported transfer.
#[derive(Clone, Debug, Serialize)]
pub struct PartialWorldBlock {
    /// Original native machine-state ID.
    pub state: usize,
    /// Current, stale, or absent transfer evidence.
    pub coverage: PartialBlockCoverage,
    /// Entry parameters whose inputs cover only supported transitions.
    pub phis: Vec<FramePhi>,
    /// Recorded attempts; never hypothetical instructions beyond the prefix.
    pub instructions: Vec<PartialInstruction>,
    /// Observed stacks, including consumed operands of a pending instruction.
    /// For stale/unexecuted coverage these name only the current entry; they
    /// are not an observed exit and cannot support an outgoing transition.
    pub exit_frames: Vec<Vec<ValueId>>,
    /// Entry effect bundle with the supported incoming transitions.
    pub effect: EffectPhi,
    /// Last observed effect; does not describe execution beyond a frontier.
    /// A stale/unexecuted block retains only its entry effect here.
    pub exit_effect: EffectId,
    /// Original incoming edge IDs that lack a supported SSA transition.
    pub open_incoming: Vec<usize>,
    /// Whether the whole analysis and this block's incoming edge coverage are closed.
    pub incoming_complete: bool,
}

/// Why an accumulated graph edge cannot be attached to the current prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DeferredEdgeReason {
    /// Its source has never been transferred.
    SourceUnexecuted,
    /// Its source entry grew after the retained transfer.
    SourceStale,
    /// The current receipt did not emit this historical transition.
    NotInExecutionEvidence,
    /// Its source stopped before a completed/control instruction supported an edge.
    SourcePendingInstruction,
}

/// An observed analysis edge whose SSA arguments are deliberately unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeferredSsaEdge {
    /// Original index into `WorldAnalysis::edges()`.
    pub edge: usize,
    /// Native source state.
    pub from: usize,
    /// Native destination state.
    pub to: usize,
    /// Recorded transition kind; this edge is not converted to a no-op.
    pub kind: MachineEdgeKind,
    /// Explicit reason SSA coverage is open on this edge.
    pub reason: DeferredEdgeReason,
}

/// Structurally verified names for a recorded graph, with all coverage gaps retained.
///
/// This artifact never satisfies the stricter `WorldSsa` contract. Its transition
/// IDs are original analysis edge IDs, so a sparse transition vector must not be
/// indexed by those IDs. Phis and deferred edges use the same original namespace.
#[derive(Clone, Debug, Serialize)]
pub struct PartialWorldSsa {
    status: Status,
    blocks: Vec<PartialWorldBlock>,
    transitions: Vec<Transition>,
    deferred_edges: Vec<DeferredSsaEdge>,
    frontiers: Vec<MachineFrontier>,
    value_count: usize,
    effect_count: usize,
}

impl PartialWorldSsa {
    /// Unmodified completion status of the underlying analysis.
    pub fn status(&self) -> Status {
        self.status
    }
    /// Blocks indexed by their original native state ID.
    pub fn blocks(&self) -> &[PartialWorldBlock] {
        &self.blocks
    }
    /// Supported transitions, in original edge order; `edge` is the original ID.
    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }
    /// Accumulated edges deliberately left without fabricated arguments or effects.
    pub fn deferred_edges(&self) -> &[DeferredSsaEdge] {
        &self.deferred_edges
    }
    /// All original resource and model frontiers, including unknown call targets.
    pub fn frontiers(&self) -> &[MachineFrontier] {
        &self.frontiers
    }
    /// Number of contiguous, unique SSA value definitions.
    pub fn value_count(&self) -> usize {
        self.value_count
    }
    /// Number of contiguous, unique effect definitions.
    pub fn effect_count(&self) -> usize {
        self.effect_count
    }
    /// Verify phases, currentness, covered and deferred edges, and retained frontiers.
    pub fn verify(&self, analysis: &WorldAnalysis) -> Result<(), SsaError> {
        verify::verify(self, analysis)
    }
}

/// Build only execution-evidenced prefixes; preserve `Incomplete` and every frontier.
pub fn build_partial_world(analysis: &WorldAnalysis) -> Result<PartialWorldSsa, SsaError> {
    let ir = build::build(analysis)?;
    ir.verify(analysis)?;
    Ok(ir)
}

fn invariant(message: impl Into<String>) -> SsaError {
    SsaError::Invariant(message.into())
}

fn is_call(op: u8) -> bool {
    use revm_bytecode::opcode;
    matches!(
        op,
        opcode::CALL
            | opcode::CALLCODE
            | opcode::DELEGATECALL
            | opcode::STATICCALL
            | opcode::CREATE
            | opcode::CREATE2
    )
}

fn coverage(state: &crate::analysis::MachineState) -> PartialBlockCoverage {
    match state.execution_evidence() {
        None => PartialBlockCoverage::Unexecuted,
        Some(evidence) if !evidence.is_current() => PartialBlockCoverage::Stale,
        Some(_) => PartialBlockCoverage::Current,
    }
}

fn deferred_reason(
    analysis: &WorldAnalysis,
    edge: &crate::analysis::MachineEdge,
) -> Option<DeferredEdgeReason> {
    let state = &analysis.states()[edge.from];
    match coverage(state) {
        PartialBlockCoverage::Unexecuted => return Some(DeferredEdgeReason::SourceUnexecuted),
        PartialBlockCoverage::Stale => return Some(DeferredEdgeReason::SourceStale),
        PartialBlockCoverage::Current => {}
    }
    let evidence = state.execution_evidence().expect("current receipt exists");
    if !evidence.successors().contains(&(edge.to, edge.kind)) {
        return Some(DeferredEdgeReason::NotInExecutionEvidence);
    }
    if evidence.instructions().last().is_some_and(|phase| {
        matches!(
            phase,
            InstructionProgress::Started | InstructionProgress::OperandsConsumed
        )
    }) {
        return Some(DeferredEdgeReason::SourcePendingInstruction);
    }
    None
}
