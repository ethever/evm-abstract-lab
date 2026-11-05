//! Typed semantic projection over an unchanged primitive witness.
//!
//! Expressions are a topologically ordered arena, scoped to one source block.
//! Primitive statements bind opaque word results once; memory/effect tokens and
//! resource behavior belong to the source IR. Compact tuples are logical values,
//! never evidence about the physical EVM stack.
use crate::{EntityId, U256};
use serde::{Deserialize, Serialize};

mod codec;
mod evaluate;
mod render;
mod validate;
pub use render::{LiftedRenderError, LiftedRenderIndex};

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
/// Predicate binding for a source structured If or conditional While.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedControl {
    /// Independent semantic control entity.
    pub id: EntityId,
    /// Source structured node; topology and loop polarity are reused explicitly.
    pub node: EntityId,
    /// Typed truth expression, evaluated in its source block scope.
    pub condition: EntityId,
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
}
impl std::fmt::Display for LiftedModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid lifted semantic arena: {self:?}")
    }
}
impl std::error::Error for LiftedModelError {}

impl LiftedProgram {
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
    }
}
