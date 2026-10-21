//! Names for recorded machine prefixes without claiming closed execution coverage.
//!
//! Only current execution receipts support instructions and transitions. Stale
//! prefixes and unsupported historical edges remain explicit gaps. Instruction
//! phases distinguish observed partial effects from completed opcode semantics;
//! unknown calls never acquire a fabricated result or return transition.

use super::{EffectId, Instruction, SsaError, Transition};
use crate::analysis::{
    EvmMachineFrontier, EvmWorldAnalysis, InstructionProgress, MachineEdgeKind, Status,
};
use crate::domain::WordElement;
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
pub type PartialWorldBlock = super::block::Block<PartialInstruction>;

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
    /// Original index into `EvmWorldAnalysis::edges()`.
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
pub struct PartialWorldSsa<N: WordElement> {
    status: Status,
    blocks: Vec<PartialWorldBlock>,
    transitions: Vec<Transition>,
    deferred_edges: Vec<DeferredSsaEdge>,
    frontiers: Vec<EvmMachineFrontier<N>>,
    value_count: usize,
    effect_count: usize,
}

impl<N: WordElement> PartialWorldSsa<N> {
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
    pub fn frontiers(&self) -> &[EvmMachineFrontier<N>] {
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
    pub fn verify(
        &self,
        analysis: &EvmWorldAnalysis<N>,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<(), SsaError> {
        verify::verify(self, analysis, budget)
    }
}

/// Build only execution-evidenced prefixes; preserve `Incomplete` and every frontier.
pub fn build_partial_world<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<PartialWorldSsa<N>, SsaError> {
    crate::ssa::checkpoint()?;
    let ir = build::build(analysis, budget)?;
    ir.verify(analysis, budget)?;
    Ok(ir)
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

fn coverage<N: WordElement>(state: &crate::analysis::EvmMachineState<N>) -> PartialBlockCoverage {
    match state.execution_evidence() {
        None => PartialBlockCoverage::Unexecuted,
        Some(evidence) if !evidence.is_current() => PartialBlockCoverage::Stale,
        Some(_) => PartialBlockCoverage::Current,
    }
}

fn deferred_reason<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
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

/// Preserve every native frontier, including range operands and shared payload roots.
/// Only range operands own N; machine payloads remain immutable Arc handles.
pub(crate) fn clone_frontiers<N: WordElement>(
    frontiers: &[EvmMachineFrontier<N>],
    budget: &mut crate::resource::WorkBudget,
) -> Result<Vec<EvmMachineFrontier<N>>, SsaError> {
    let mut result = Vec::with_capacity(frontiers.len());
    for frontier in frontiers {
        let mut work = frontier
            .target
            .as_ref()
            .map_or(1, |key| key.frames.len().saturating_mul(8));
        if let Some(range) = &frontier.range {
            for value in [
                &range.request.offset,
                &range.request.size,
                &range.request.limit,
                &range.condition,
            ] {
                work = work.saturating_add(value.clone_work());
            }
        }
        super::pay(budget, work)?;
        result.push(frontier.clone());
        super::pay(budget, 0)?;
    }
    Ok(result)
}

/// Range evidence retains its actual operands and immutable machine identity.
/// Comparing only a frontier's location would miss lost source assumptions.
pub(crate) fn same_frontiers<N: WordElement>(
    actual: &[EvmMachineFrontier<N>],
    expected: &[EvmMachineFrontier<N>],
    budget: &mut crate::resource::WorkBudget,
) -> Result<bool, SsaError> {
    use std::sync::Arc;
    if actual.len() != expected.len() {
        return Ok(false);
    }
    for (a, b) in actual.iter().zip(expected) {
        super::pay(budget, 0)?;
        if a.from != b.from || a.target != b.target || a.pc != b.pc || a.reason != b.reason {
            return Ok(false);
        }
        let shared = match (&a.payload, &b.payload) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !shared {
            return Ok(false);
        }
        match (&a.range, &b.range) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                if a.nonzero != b.nonzero
                    || !Arc::ptr_eq(&a.payload, &b.payload)
                    || a.request.kind != b.request.kind
                {
                    return Ok(false);
                }
                for (a, b) in [
                    (&a.request.offset, &b.request.offset),
                    (&a.request.size, &b.request.size),
                    (&a.request.limit, &b.request.limit),
                    (&a.condition, &b.condition),
                ] {
                    super::pay(budget, a.work_size().saturating_add(b.work_size()))?;
                    if a != b {
                        return Ok(false);
                    }
                    super::pay(budget, 0)?;
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}
