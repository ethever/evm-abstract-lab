//! One instruction-preserving SSA with explicit value semantics and scoped facts.
//!
//! Construction first validates the execution receipts with the original SSA
//! verifier. Normalization then changes only value computation: native states,
//! edges, phis, fault phases and the complete effect chain remain present. An
//! instruction's `origin` is bytecode evidence, never a replacement opcode.
//! Open-input observations remain visible facts and cannot justify a rewrite.

use super::{DeferredSsaEdge, EffectId, Instruction, SsaError, Transition, ValueId};
use crate::domain::WordElement;
use crate::{
    analysis::{EvmAnalysis, EvmMachineFrontier, EvmWorldAnalysis, InstructionProgress, Status},
    domain::provenance::Provenance,
};
use alloy_primitives::U256;
use serde::Serialize;
use thiserror::Error;

mod build;
mod simplify;
#[cfg(test)]
mod tests;
mod verify;

/// A computation operand, after justified constant and copy propagation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ValueOperand {
    /// A retained SSA definition.
    Value(ValueId),
    /// An exact word justified for this use.
    Constant(U256),
}

/// The computation on a retained instruction row. Constant and Copy each
/// retain the original row's unique normal result definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum NormalizedOperation {
    /// Original opcode semantics with justified operand substitutions.
    Opcode {
        /// Original bytecode opcode.
        opcode: u8,
        /// Arguments in EVM pop order.
        operands: Vec<ValueOperand>,
    },
    /// Exact result, retaining the original instruction's effects.
    Constant {
        /// The word assigned to the retained result definition.
        value: U256,
    },
    /// Unconditional equality with an existing value, retaining a new result ID.
    Copy {
        /// The equal operand.
        value: ValueOperand,
    },
}

/// One original instruction attempt and its final value computation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NormalizedInstruction {
    /// Bytecode identity, stack uses and retained definitions, with final IDs.
    #[serde(flatten)]
    pub origin: Instruction,
    /// Phase actually reached, independent of whole-graph completion.
    pub progress: InstructionProgress,
    /// Bundle before this attempt.
    pub effect_input: EffectId,
    /// Observed bundle after this attempt; absent for Started.
    pub effect_result: Option<EffectId>,
    /// Final value semantics, separate from bytecode evidence.
    pub operation: NormalizedOperation,
}

/// A native machine state; all frame parameters and coverage gaps are retained.
pub type NormalizedBlock = super::block::Block<NormalizedInstruction>;

/// Where a fact applies. Definition facts cover every input represented by
/// that SSA definition. State observations describe only its current receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FactScope {
    /// A guarantee for every represented input to this definition.
    Definition,
    /// A current joined-entry or execution observation with open coverage.
    State {
        /// Native state supplying the observation.
        state: usize,
    },
}

/// Currentness of the collection supplying this fact, separate from its scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FactValidity {
    /// Collection describes the current state entry.
    Current,
    /// No current instruction or transfer observation supplies the property.
    Uncollected,
    /// Retained transfer receipt predates the state's current joined entry.
    Stale,
}

/// The evidence supplying numeric properties, independent of source categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FactSource {
    /// Joined machine entry stack.
    Entry,
    /// Numeric and provenance projection captured at instruction completion.
    Instruction,
    /// Destination entry guarantees for a supported continuation result.
    Transition,
    /// Explicit PUSH immediate.
    Literal,
    /// Exact shared EVM evaluation of proven constant operands.
    ConstantFold,
    /// An unconditional EVM identity using constants or the same SSA value.
    AlgebraicIdentity,
}

/// A numeric or provenance observation. Multiple facts for a value preserve an
/// observed source separately from a definition-wide arithmetic proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValueFact<N: WordElement> {
    /// Final SSA definition receiving this property.
    pub value: ValueId,
    /// The boundary within which the property is valid.
    pub scope: FactScope,
    /// Currentness of the observation or arithmetic proof.
    pub validity: FactValidity,
    /// Evidence producing this property.
    pub source: FactSource,
    /// Retained actual native observation. Independent derived word proofs
    /// deliberately omit a redundant numeric carrier.
    pub numeric: Option<N>,
    /// Possible source categories; they do not establish equal values.
    pub provenance: Option<Provenance>,
    /// Exact native singleton or independently derived EVM word proof.
    pub constant: Option<U256>,
}

/// An original definition site, using native state and edge IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ValueOrigin {
    /// An unchanged frame entry parameter.
    Phi {
        /// Native state ID.
        state: usize,
        /// Frame index, oldest caller first.
        frame: usize,
        /// Stack slot, bottom to top.
        slot: usize,
    },
    /// A retained ordinary result definition.
    Instruction {
        /// Native state ID.
        state: usize,
        /// Index into the state's instruction receipt.
        index: usize,
        /// Original bytecode position.
        pc: usize,
        /// Original bytecode opcode.
        opcode: u8,
    },
    /// A deferred CALL or CREATE result.
    Transition {
        /// Original native edge ID.
        edge: usize,
    },
}

/// Why a retained result's computation has its final form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ValueTransformation {
    /// Retains opcode semantics, possibly with propagated operands.
    Original,
    /// Assigns a PUSH immediate.
    Literal,
    /// Assigns exact shared EVM evaluation of constant operands.
    ConstantFold,
    /// Assigns an unconditional identity or constant.
    AlgebraicIdentity,
    /// Assigns a singleton from complete current definition evidence.
    AnalyzedConstant,
}

/// Independent origin, equivalence and computation dependencies for one final
/// value. Equal numeric summaries or source categories never create aliases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValueMapping {
    /// Final SSA definition, also this mapping's index.
    pub value: ValueId,
    /// Temporary raw SSA ID before final naming.
    pub original_value: ValueId,
    /// Original definition site, independent of simplification.
    pub origin: ValueOrigin,
    /// An unconditional copy replacement; the definition itself is retained.
    pub equivalent: Option<ValueId>,
    /// Original computation inputs with final IDs, even when folded to a
    /// constant. Semantic uses are separate on the normalized operation.
    pub dependencies: Vec<ValueId>,
    /// Proof category for this definition's final computation.
    pub transformation: ValueTransformation,
}

/// Validated, instruction-preserving SSA shared by CLI and server consumers.
#[derive(Clone, Debug, Serialize)]
pub struct NormalizedSsa<N: WordElement> {
    status: Status,
    complete: bool,
    blocks: Vec<NormalizedBlock>,
    transitions: Vec<Transition>,
    deferred_edges: Vec<DeferredSsaEdge>,
    frontiers: Vec<EvmMachineFrontier<N>>,
    value_count: usize,
    effect_count: usize,
    facts: Vec<ValueFact<N>>,
    mappings: Vec<ValueMapping>,
}

impl<N: WordElement> NormalizedSsa<N> {
    /// Unmodified underlying analysis status.
    pub fn status(&self) -> Status {
        self.status
    }
    /// True only after closed-world SSA validation succeeds.
    pub fn complete(&self) -> bool {
        self.complete
    }
    /// Blocks in native state order.
    pub fn blocks(&self) -> &[NormalizedBlock] {
        &self.blocks
    }
    /// Supported transitions in original edge order; their IDs may be sparse.
    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }
    /// Original edges with explicit unsupported-coverage reasons.
    pub fn deferred_edges(&self) -> &[DeferredSsaEdge] {
        &self.deferred_edges
    }
    /// Every original resource and model frontier.
    pub fn frontiers(&self) -> &[EvmMachineFrontier<N>] {
        &self.frontiers
    }
    /// Number of contiguous retained value definitions.
    pub fn value_count(&self) -> usize {
        self.value_count
    }
    /// Number of contiguous unchanged effect definitions.
    pub fn effect_count(&self) -> usize {
        self.effect_count
    }
    /// Scoped observations and definition proofs; a value may have several.
    pub fn facts(&self) -> &[ValueFact<N>] {
        &self.facts
    }
    /// One mapping per definition, indexed by final ValueId.
    pub fn mappings(&self) -> &[ValueMapping] {
        &self.mappings
    }

    /// Recheck origins, coverage, value scopes and normalization proofs against
    /// this execution. The temporary raw SSA is used only for validation.
    pub fn verify(
        &self,
        analysis: &EvmWorldAnalysis<N>,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<(), SsaError> {
        verify::verify(self, analysis, budget)
    }
}

/// A normalized artifact's violated contract; original verifier errors retain
/// their existing, stricter opcode and execution-receipt contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Error)]
pub enum NormalizationInvariantKind {
    /// Bytecode identity, operands, parameters or exit stacks changed.
    #[error("normalized origin differs from verified execution")]
    Origin,
    /// The final value namespace contains gaps or repeated definitions.
    #[error("normalized values are not contiguous unique definitions")]
    ValueNames,
    /// A rewritten computation lacks current definition-wide evidence.
    #[error("normalized operation lacks a valid computation proof")]
    Operation,
    /// A property differs from the observation or proof it claims.
    #[error("normalized fact exceeds its value scope or current evidence")]
    Facts,
    /// A source, alias or original computation dependency is incorrect.
    #[error("normalized origin, equivalence or dependency mapping is invalid")]
    Mappings,
    /// An instruction's original effect chain changed.
    #[error("normalization changed instruction effects")]
    Effects,
    /// An original call, return or local transition changed.
    #[error("normalization changed a native transition")]
    Transitions,
    /// Status, phases, currentness or graph coverage changed.
    #[error("normalization changed execution coverage")]
    Coverage,
}

/// A typed normalization contract failure with native coordinates when known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Error)]
#[error("{kind} (state={state:?}, pc={pc:?}, value={value:?})")]
pub struct NormalizationInvariant {
    /// Exact normalization contract that failed.
    pub kind: NormalizationInvariantKind,
    /// Native state containing the failure.
    pub state: Option<usize>,
    /// Original opcode position.
    pub pc: Option<usize>,
    /// Retained final value definition affected by the failure.
    pub value: Option<ValueId>,
}

fn invariant(kind: NormalizationInvariantKind) -> SsaError {
    SsaError::Normalization(Box::new(NormalizationInvariant {
        kind,
        state: None,
        pc: None,
        value: None,
    }))
}

/// Normalize the native machine underlying a converged single-program view.
/// Returned state IDs belong to `EvmAnalysis::execution()`, including child frames.
pub fn build_normalized<N: WordElement>(
    analysis: &EvmAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<NormalizedSsa<N>, SsaError> {
    build_normalized_world(analysis.execution(), budget)
}

/// Build a complete normalized artifact after the stronger world SSA verifier.
pub fn build_normalized_world<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<NormalizedSsa<N>, SsaError> {
    super::pay(budget, 0)?;
    super::world::build(analysis, Some(&mut *budget))?;
    let raw = super::build_partial_world(analysis, budget)?;
    let ir = build::normalize(&raw, analysis, true, budget)?;
    verify::against_raw(&ir, &raw, analysis, budget)?;
    Ok(ir)
}

/// Build only supported execution prefixes. A converged input also receives
/// closed-world verification before its normalized artifact claims completeness.
pub fn build_normalized_partial_world<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<NormalizedSsa<N>, SsaError> {
    super::pay(budget, 0)?;
    if analysis.status() == Status::Converged {
        return build_normalized_world(analysis, budget);
    }
    let raw = super::build_partial_world(analysis, budget)?;
    let ir = build::normalize(&raw, analysis, false, budget)?;
    verify::against_raw(&ir, &raw, analysis, budget)?;
    Ok(ir)
}
