//! Typed semantic projection over an unchanged primitive witness.
//!
//! Expressions are a topologically ordered arena, scoped to one source block.
//! Primitive statements bind opaque word results once; memory/effect tokens and
//! resource behavior belong to the source IR. Compact tuples are logical values,
//! never evidence about the physical EVM stack.
use crate::{EntityId, U256};
use serde::{Deserialize, Serialize};

pub(crate) mod codec;
mod evaluate;
mod payload;
pub(crate) mod render;
pub(crate) mod sources;
mod validate;
mod word;
pub use payload::{LiftedConstruction, LiftedErrorPayload, LiftedRevert, MAX_ERROR_PAYLOAD_BYTES};
pub use render::{
    FailureFirstIf, LiftedBranchLayout, LiftedBranchLayouts, LiftedIfPlan, LiftedIfSuffix,
    LiftedRenderError, LiftedRenderIndex, RequireIf,
};
pub use word::{WordExpr, WordRewrite, WordSimplifyError, simplify_word};

/// Hard transport admission ceiling, independent of an attempt's smaller limits.
pub const MAX_LIFTED_RECORDS: usize = 1_000_000;
/// Maximum expanded expression text; shared DAGs must not cause exponential output.
pub const MAX_LIFTED_EXPRESSION_TEXT: usize = 65_536;

/// Words and branch truth have different identities and conversion semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiftedType {
    /// EVM modulo-2^256 word.
    Word256,
    /// Boolean truth, distinct from an arbitrary nonzero word.
    Predicate,
}
/// Explicit signedness of a word comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiftedRelation {
    /// Equal words.
    Eq,
    /// Unequal words.
    Ne,
    /// Unsigned less than.
    Ult,
    /// Unsigned less than or equal.
    Ule,
    /// Unsigned greater than.
    Ugt,
    /// Unsigned greater than or equal.
    Uge,
    /// Signed less than.
    Slt,
    /// Signed less than or equal.
    Sle,
    /// Signed greater than.
    Sgt,
    /// Signed greater than or equal.
    Sge,
}
impl LiftedRelation {
    /// Logical complement, preserving signedness.
    pub fn complement(self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Ult => Self::Uge,
            Self::Ule => Self::Ugt,
            Self::Ugt => Self::Ule,
            Self::Uge => Self::Ult,
            Self::Slt => Self::Sge,
            Self::Sle => Self::Sgt,
            Self::Sgt => Self::Sle,
            Self::Sge => Self::Slt,
        }
    }
    /// Fixed-width two's-complement comparison.
    pub fn evaluate(self, a: U256, b: U256) -> bool {
        let signed_lt = if a.bit(255) == b.bit(255) {
            a < b
        } else {
            a.bit(255)
        };
        match self {
            Self::Eq => a == b,
            Self::Ne => a != b,
            Self::Ult => a < b,
            Self::Ule => a <= b,
            Self::Ugt => a > b,
            Self::Uge => a >= b,
            Self::Slt => signed_lt,
            Self::Sle => signed_lt || a == b,
            Self::Sgt => !signed_lt && a != b,
            Self::Sge => !signed_lt,
        }
    }
}
/// Flat, non-recursive expression. Every operand is an earlier arena record.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiftedExpressionKind {
    /// Exact word literal.
    Word(U256),
    /// Exact truth literal.
    Predicate(bool),
    /// Opaque primitive result or block entry binding in the source witness.
    Value(EntityId),
    /// Explicit EVM numeric encoding (false=0, true=1).
    WordFromPredicate(EntityId),
    /// Explicit truth test of an arbitrary word.
    NonZero(EntityId),
    /// Logical negation, only for predicates.
    Not(EntityId),
    /// Word comparison with explicit signedness.
    Compare {
        /// Comparison relation.
        relation: LiftedRelation,
        /// First popped word.
        left: EntityId,
        /// Second popped word.
        right: EntityId,
    },
    /// Total pure EVM arithmetic in original pop order.
    WordOp {
        /// Supported pure word opcode.
        opcode: u8,
        /// Word operands in pop order.
        arguments: Vec<EntityId>,
    },
}
impl LiftedExpressionKind {
    /// Direct arena operands, excluding raw witness bindings.
    pub fn operands(&self) -> Vec<EntityId> {
        match self {
            Self::WordFromPredicate(id) | Self::NonZero(id) | Self::Not(id) => vec![*id],
            Self::Compare { left, right, .. } => vec![*left, *right],
            Self::WordOp { arguments, .. } => arguments.clone(),
            _ => Vec::new(),
        }
    }
    /// Result type without inspecting any child expression.
    pub fn ty(&self) -> LiftedType {
        match self {
            Self::Predicate(_) | Self::NonZero(_) | Self::Not(_) | Self::Compare { .. } => {
                LiftedType::Predicate
            }
            _ => LiftedType::Word256,
        }
    }
}
strict_record! {
/// One artifact entity in the flat semantic arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedExpression {
    /// Independent artifact-local expression identity.
    pub id: EntityId,
    /// Source evaluation scope; operands cannot cross block iterations.
    pub block: EntityId,
    /// Checked result sort.
    pub ty: LiftedType,
    /// Typed expression semantics.
    pub kind: LiftedExpressionKind,
}
}
strict_record! {
/// An ordered primitive invocation, linked to its unchanged witness operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedStatement {
    /// Independent semantic statement entity.
    pub id: EntityId,
    /// Source operation defining effects, memory dependencies and fault behavior.
    pub operation: EntityId,
    /// Semantic word input expressions in pop order.
    pub inputs: Vec<EntityId>,
    /// Live raw word result bindings in source order. Other result slots are
    /// discarded here; the witness still records every primitive output.
    pub outputs: Vec<EntityId>,
}
}
strict_record! {
/// Logical block bindings and control operands over source topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedBlock {
    /// Independent semantic block entity.
    pub id: EntityId,
    /// Source block whose primitive order and topology remain the witness.
    pub block: EntityId,
    /// Live logical entry bindings in source order, not physical stack slots.
    pub parameters: Vec<EntityId>,
    /// Ordered residual/effectful primitive invocations.
    pub statements: Vec<LiftedStatement>,
    /// Predicate for a source conditional terminator.
    pub condition: Option<EntityId>,
    /// Word for indirect, partial or potentially invalid target selection.
    pub target: Option<EntityId>,
    /// Semantic terminal word operands in source order.
    pub exit_values: Vec<EntityId>,
}
}
strict_record! {
/// Simultaneous logical assignment along an unchanged source edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedTransfer {
    /// Independent semantic transfer entity.
    pub id: EntityId,
    /// Source edge; its memory/effect arguments are preserved in the witness.
    pub edge: EntityId,
    /// Logical RHS expressions, aligned with destination LiftedBlock.parameters.
    pub arguments: Vec<EntityId>,
}
}
strict_record! {
/// A shared, well-founded exact-payload failure fact over a source block.
/// This is metadata on the existing witness, not a new artifact entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedFailure {
    /// Source block in the unchanged primitive witness.
    pub block: EntityId,
    /// Zero for a certified terminal REVERT; otherwise one plus the largest successor rank.
    pub rank: u32,
    /// Exact byte payload identity; legacy schema-3 facts describe empty REVERT.
    #[serde(skip_serializing_if = "LiftedGuardPayload::is_empty")]
    pub payload: LiftedGuardPayload = LiftedGuardPayload::EmptyRevert,
}
}
/// Mechanically certified failure payload semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiftedGuardPayload {
    /// REVERT whose size is exactly zero, independent of its offset.
    EmptyRevert,
    /// Canonical Error(string) byte payload in the shared payload arena.
    ErrorString(EntityId),
}
impl LiftedGuardPayload {
    /// Whether this is the legacy zero-length payload.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::EmptyRevert)
    }
}
strict_record! {
/// Occurrence-local following failure suffix, never a global node deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedGuardContinuation {
    /// Exact containing source Sequence.
    pub sequence: EntityId,
    /// Inclusive child position immediately after the source If.
    pub start: usize,
    /// Exclusive end of that Sequence.
    pub end: usize,
    /// Conservative normal fallthrough of the raw successful fragment. False
    /// certifies a local structured cut; unknown escapes conservatively use true.
    pub pass_falls_through: bool,
}
}
strict_record! {
/// Certified formatting overlay attached to an existing source If occurrence.
/// The successful arm retains its original simultaneous edge assignments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedGuard {
    /// Source conditional block.
    pub block: EntityId,
    /// Whether nonzero source truth is the successful route.
    pub pass_nonzero: bool,
    /// Successful raw edge, evaluated only after require.
    pub pass_edge: EntityId,
    /// Failed raw edge into the shared failure proof.
    pub failure_edge: EntityId,
    /// Exact raw successful If arm root to render after require.
    pub pass_node: EntityId,
    /// Exact raw failed If arm root, retained for inspection.
    pub failure_node: EntityId,
    /// Proven failure payload category.
    pub payload: LiftedGuardPayload,
    /// Plan for an immediate following Sequence suffix. An inherited ancestor
    /// suffix has no omission plan and remains visible for all other arms.
    pub continuation: Option<LiftedGuardContinuation>,
}
}
impl LiftedGuard {
    /// Direct witness occurrences. Following edges/nodes reaches the shared
    /// failure route without copying all transitive failure ancestry per guard.
    pub fn source_references(&self) -> impl Iterator<Item = EntityId> + '_ {
        [
            self.block,
            self.pass_edge,
            self.failure_edge,
            self.pass_node,
            self.failure_node,
        ]
        .into_iter()
        .chain(self.continuation.as_ref().map(|c| c.sequence))
    }
}
strict_record! {
/// Predicate binding for a source structured If or conditional While.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedControl {
    /// Independent semantic control entity.
    pub id: EntityId,
    /// Source structured node; topology and loop polarity are reused explicitly.
    pub node: EntityId,
    /// Typed truth expression, evaluated in its source block scope.
    pub condition: EntityId,
    /// Optional certified guard using this same semantic and source identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<LiftedGuard> = None,
}
}
/// Immutable high-level projection, certified against its source primitive IR.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LiftedProgram {
    /// Topologically ordered, non-recursive semantic DAG.
    pub expressions: Vec<LiftedExpression>,
    /// Semantic bodies and terminator operands.
    pub blocks: Vec<LiftedBlock>,
    /// Synchronously projected edge arguments.
    pub transfers: Vec<LiftedTransfer>,
    /// Typed conditions attached to reused structured control topology.
    pub controls: Vec<LiftedControl>,
    /// Shared ranked failure facts; no per-guard transitive closure copies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<LiftedFailure>,
    /// Shared byte identities, independent of source construction identities.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub payloads: Vec<LiftedErrorPayload>,
    /// Terminal byte certificates over exact primitive memory versions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reverts: Vec<LiftedRevert>,
    /// Exact source statement ranges retaining lazy original fault semantics.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constructions: Vec<LiftedConstruction>,
    /// Canonical source code definitions, checked independently of discovery.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub procedures: Vec<crate::SharedProcedure>,
    /// Exact source occurrences and checked expansion correspondences. Nested
    /// definitions share subregions without duplicating their source witness.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<crate::SharedInvocation>,
}
/// A typed evaluation result; callers cannot confuse word 2 with truth true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftedValue {
    /// Modulo-2^256 numeric result.
    Word(U256),
    /// Logical result.
    Predicate(bool),
}
/// Transport-level DAG admission failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiftedModelError {
    /// A flat projection exceeds the fixed transport ceiling.
    RecordLimit,
    /// A record identity is zero or repeated.
    DuplicateIdentity(EntityId),
    /// An operand is absent, forward, cyclic or in a different evaluation scope.
    Operand(EntityId),
    /// A result/operand sort or pure opcode arity is invalid.
    Type(EntityId),
    /// Malformed guard overlay or shared failure metadata.
    Guard(EntityId),
    /// A guard target or suffix does not match its owned source occurrence.
    GuardSource(EntityId),
    /// Invalid bounded payload, terminal witness, or construction reference.
    Payload(EntityId),
    /// Malformed bounded procedure interface, map, dependency or invocation.
    Procedure(EntityId),
}
impl std::fmt::Display for LiftedModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid lifted semantic arena: {self:?}")
    }
}
impl std::error::Error for LiftedModelError {}

impl LiftedProgram {
    /// Whether schema-5 procedure semantics are present.
    pub fn has_procedures(&self) -> bool {
        !self.procedures.is_empty() || !self.invocations.is_empty()
    }
    /// Whether schema-4 payload or construction semantics are present.
    pub fn has_payloads(&self) -> bool {
        !self.payloads.is_empty()
            || !self.reverts.is_empty()
            || !self.constructions.is_empty()
            || self.failures.iter().any(|f| !f.payload.is_empty())
            || self
                .controls
                .iter()
                .filter_map(|c| c.guard.as_ref())
                .any(|g| !g.payload.is_empty())
    }
    /// Whether the artifact uses schema-3 guard semantics or their proof table.
    pub fn has_guards(&self) -> bool {
        !self.failures.is_empty() || self.controls.iter().any(|c| c.guard.is_some())
    }
    /// Transport/history record units, including metadata without new identities.
    pub fn record_count(&self) -> usize {
        self.entity_ids()
            .count()
            .saturating_add(self.procedures.iter().fold(0_usize, |n, p| {
                n.saturating_add(p.interface.record_count())
                    .saturating_add(2)
            }))
            .saturating_add(self.invocations.iter().fold(0_usize, |n, i| {
                n.saturating_add(i.bindings.len().saturating_mul(2))
                    .saturating_add(5)
            }))
            .saturating_add(self.failures.len())
            .saturating_add(self.payloads.iter().fold(0_usize, |n, p| {
                n.saturating_add(p.message.len())
                    .saturating_add(p.bytes.len())
            }))
            .saturating_add(self.reverts.iter().fold(0_usize, |n, r| {
                n.saturating_add(r.writes.len()).saturating_add(9)
            }))
            .saturating_add(self.constructions.len().saturating_mul(5))
            .saturating_add(
                self.controls
                    .iter()
                    .filter_map(|c| c.guard.as_ref())
                    .map(|g| 1 + usize::from(g.continuation.is_some()))
                    .sum::<usize>(),
            )
    }
    /// Lookup convenience; callers rendering many rows can build their own index.
    pub fn expression(&self, id: EntityId) -> Option<&LiftedExpression> {
        self.expressions.iter().find(|e| e.id == id)
    }
    /// Find semantic bindings for a source block.
    pub fn block(&self, source: EntityId) -> Option<&LiftedBlock> {
        self.blocks.iter().find(|b| b.block == source)
    }
    /// Find projected arguments for a source edge.
    pub fn transfer(&self, source: EntityId) -> Option<&LiftedTransfer> {
        self.transfers.iter().find(|e| e.edge == source)
    }
    /// Find the predicate for a source structured condition.
    pub fn control(&self, source: EntityId) -> Option<&LiftedControl> {
        self.controls.iter().find(|c| c.node == source)
    }
    /// All new semantic entity IDs, including statements nested in block records.
    pub fn entity_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.expressions
            .iter()
            .map(|e| e.id)
            .chain(
                self.blocks
                    .iter()
                    .flat_map(|b| std::iter::once(b.id).chain(b.statements.iter().map(|s| s.id))),
            )
            .chain(self.transfers.iter().map(|t| t.id))
            .chain(self.controls.iter().map(|c| c.id))
            .chain(self.payloads.iter().map(|p| p.id))
            .chain(self.reverts.iter().map(|r| r.id))
            .chain(self.constructions.iter().map(|c| c.id))
            .chain(self.procedures.iter().map(|p| p.id))
            .chain(self.invocations.iter().map(|i| i.id))
    }
}
