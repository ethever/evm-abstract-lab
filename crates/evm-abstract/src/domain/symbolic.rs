//! Immutable, scoped EVM expressions. Identity is structural, never a bytecode PC.
//!
//! Node counts bound the expanded tree, a conservative upper bound on DAG work.
//! Runtime leaves are process-fresh; input leaves retain caller-supplied scopes.

use crate::domain::identity::Symbol;
use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use thiserror::Error;

mod bytes;
mod graph;
mod hash;
mod materialize;
mod reads;
mod state;
mod storage;
#[cfg(test)]
mod tests;
mod walk;
pub use bytes::{ByteStateId, ByteStateKind, ByteView};
pub use graph::{GraphBinding, GraphLimits, GraphSource, SymbolicGraph, SymbolicGraphNode};
pub use hash::{HashSupport, MAX_NORMALIZED_HASH_BYTES};
pub use state::{SourceId, StateBinding, StateError, StateLimits};
pub use storage::{SlotStateId, SlotStateKind};
pub(crate) fn state_check(nodes: usize, depth: usize, limits: ExprLimits) -> Result<(), ExprError> {
    state::check(nodes, depth, limits.into()).map_err(Into::into)
}

static NEXT_FRESH: AtomicU64 = AtomicU64::new(1);

/// Bounds applied before constructing an operation expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ExprLimits {
    /// Expanded-tree node bound, including repeated shared children.
    pub max_nodes: usize,
    /// Maximum number of nodes on a root-to-leaf path.
    pub max_depth: usize,
}
impl Default for ExprLimits {
    fn default() -> Self {
        Self {
            max_nodes: 1024,
            max_depth: 64,
        }
    }
}

/// One immutable input within its owning environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InputAtom {
    /// Caller-owned equality namespace; names alone do not establish equality.
    /// Native graphs retain this scope within their opaque producer namespace.
    pub scope: u64,
    /// Meaning of the immutable input.
    pub symbol: Symbol,
}

/// Read-only structural expression used by typed reports and solver encoders.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ExprKind {
    /// Exact 256-bit word.
    Constant(U256),
    /// Scoped immutable environment input.
    Input(InputAtom),
    /// Fresh runtime identity; not a concrete value.
    Fresh(#[serde(skip)] u64),
    /// A pure EVM operation in pop-argument order.
    Operation {
        /// Raw opcode.
        opcode: u8,
        /// Structurally retained operands.
        args: Vec<ExprId>,
    },
    /// Nonzero EVM condition chooses the first alternative.
    Ite {
        /// Nonzero EVM condition selecting the first alternative.
        condition: ExprId,
        /// Word chosen when the condition is nonzero.
        then_value: ExprId,
        /// Word chosen when the condition is zero.
        else_value: ExprId,
    },
    /// Read a byte, zero extended to Word256, from an immutable state.
    ByteRead {
        /// Immutable backing-state reference.
        state: ByteStateId,
        /// Byte address as a Word256 expression.
        index: ExprId,
    },
    /// Big-endian 32-byte read at a relative offset in a bounded view.
    WordRead {
        /// Bounded zero-padded source sequence.
        view: ByteView,
        /// Relative first byte address.
        offset: ExprId,
    },
    /// Read an immutable storage state at a Word256 key.
    SlotRead {
        /// Immutable backing-state reference.
        state: SlotStateId,
        /// Word256 storage key.
        key: ExprId,
    },
    /// Keccak of the complete bounded, zero-padded sequence.
    Keccak {
        /// Complete bounded sequence passed to Keccak.
        input: ByteView,
    },
}
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Node {
    kind: ExprKind,
    nodes: usize,
    depth: usize,
    #[serde(skip)]
    fingerprint: u64,
    #[serde(skip)]
    state_semantics: bool,
}

/// An immutable expression handle with structural equality and cheap cloning.
///
/// Serialization exports a bounded flat graph, retaining common leaf bindings
/// across separate graphs in one report through an opaque producer namespace.
/// Exported identities do not establish semantic equality across reports.
#[derive(Clone, Debug)]
pub struct ExprId(Arc<Node>);

/// An expression cannot be retained within its explicit resource policy.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ExprError {
    /// This opcode is not a supported pure EVM word operation.
    #[error("opcode 0x{0:02x} has no pure expression semantics")]
    /// Immutable state operation within its declared resource bounds.
    Unsupported(u8),
    /// Operand count disagrees with the pure operation.
    #[error("opcode 0x{opcode:02x} expects {expected} operands, received {actual}")]
    /// Immutable state operation within its declared resource bounds.
    Arity {
        /// Opcode.
        opcode: u8,
        /// Expected operands.
        expected: usize,
        /// Received operands.
        actual: usize,
    },
    /// Retaining this expression would exceed the node/depth policy.
    #[error("expression needs {nodes} nodes and depth {depth}, exceeding its policy")]
    /// Immutable state operation within its declared resource bounds.
    Limit {
        /// Expanded node count.
        nodes: usize,
        /// Tree depth.
        depth: usize,
    },
    /// Cancellation interrupted a state graph construction or traversal.
    #[error("symbolic graph operation cancelled")]
    /// Immutable state operation within its declared resource bounds.
    Cancelled,
    /// Flat export exceeds its independent exact payload budget.
    #[error("symbolic export needs {bytes} exact bytes, exceeding limit {limit}")]
    ExportBytes {
        /// Exact payload bytes required.
        bytes: usize,
        /// Configured exact payload allowance.
        limit: usize,
    },
}

impl ExprId {
    fn leaf(kind: ExprKind) -> Self {
        let fingerprint = state::fingerprint(&kind);
        Self(Arc::new(Node {
            kind,
            nodes: 1,
            depth: 1,
            fingerprint,
            state_semantics: false,
        }))
    }
    /// Exact EVM word.
    pub fn constant(value: U256) -> Self {
        Self::leaf(ExprKind::Constant(value))
    }
    /// Same symbol and scope describe the same immutable input.
    pub fn input(scope: u64, symbol: Symbol) -> Self {
        Self::leaf(ExprKind::Input(InputAtom { scope, symbol }))
    }
    /// A fresh unknown runtime value; exhaustion conservatively declines identity.
    pub fn fresh() -> Option<Self> {
        NEXT_FRESH
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .ok()
            .map(|id| Self::leaf(ExprKind::Fresh(id)))
    }
    /// Build a pure EVM operation with operands in EVM pop order.
    pub fn operation(op: u8, args: &[Self], limits: ExprLimits) -> Result<Self, ExprError> {
        let expected = arity(op).ok_or(ExprError::Unsupported(op))?;
        if args.len() != expected {
            return Err(ExprError::Arity {
                opcode: op,
                expected,
                actual: args.len(),
            });
        }
        if limits.max_nodes == 0 || limits.max_depth == 0 {
            return Err(ExprError::Limit { nodes: 1, depth: 1 });
        }
        if let Some(values) = args
            .iter()
            .map(Self::as_constant)
            .collect::<Option<Vec<_>>>()
        {
            return Ok(Self::constant(super::concrete::evaluate(
                op,
                values[0],
                values.get(1).copied().unwrap_or(U256::ZERO),
                values.get(2).copied().unwrap_or(U256::ZERO),
            )));
        }
        let a = &args[0];
        let b = args.get(1);
        let is = |e: &Self, value| e.as_constant() == Some(value);
        let reduced = match op {
            opcode::ADD | opcode::OR | opcode::XOR if b.is_some_and(|b| is(b, U256::ZERO)) => {
                Some(a.clone())
            }
            opcode::ADD | opcode::OR | opcode::XOR if is(a, U256::ZERO) => b.cloned(),
            opcode::MUL | opcode::AND
                if is(a, U256::ZERO) || b.is_some_and(|b| is(b, U256::ZERO)) =>
            {
                Some(Self::constant(U256::ZERO))
            }
            opcode::EXP if b.is_some_and(|b| is(b, U256::ZERO)) => {
                Some(Self::constant(U256::from(1)))
            }
            opcode::EXP if b.is_some_and(|b| is(b, U256::from(1))) => Some(a.clone()),
            opcode::MUL if is(a, U256::from(1)) => b.cloned(),
            opcode::MUL if b.is_some_and(|b| is(b, U256::from(1))) => Some(a.clone()),
            opcode::AND if is(a, U256::MAX) => b.cloned(),
            opcode::AND if b.is_some_and(|b| is(b, U256::MAX)) => Some(a.clone()),
            opcode::SHL | opcode::SHR | opcode::SAR if is(a, U256::ZERO) => b.cloned(),
            opcode::SUB | opcode::XOR if b == Some(a) => Some(Self::constant(U256::ZERO)),
            opcode::EQ if b == Some(a) => Some(Self::constant(U256::from(1))),
            opcode::AND | opcode::OR if b == Some(a) => Some(a.clone()),
            opcode::NOT => match a.kind() {
                ExprKind::Operation {
                    opcode: inner,
                    args,
                } if *inner == opcode::NOT => Some(args[0].clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(reduced) = reduced {
            if reduced.nodes() > limits.max_nodes || reduced.depth() > limits.max_depth {
                return Err(ExprError::Limit {
                    nodes: reduced.nodes(),
                    depth: reduced.depth(),
                });
            }
            return Ok(reduced);
        }
        let depth = args
            .iter()
            .map(Self::depth)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ExprError::Limit {
                nodes: usize::MAX,
                depth: usize::MAX,
            })?;
        let nodes = args
            .iter()
            .try_fold(1usize, |n, a| n.checked_add(a.nodes()))
            .filter(|nodes| *nodes != usize::MAX)
            .ok_or(ExprError::Limit {
                nodes: usize::MAX,
                depth,
            })?;
        if nodes > limits.max_nodes || depth > limits.max_depth {
            return Err(ExprError::Limit { nodes, depth });
        }
        Self::retained(
            ExprKind::Operation {
                opcode: op,
                args: args.to_vec(),
            },
            nodes,
            depth,
            limits,
        )
    }
    /// Expanded node count, including shared children more than once.
    pub fn nodes(&self) -> usize {
        self.0.nodes
    }
    /// Root-to-leaf node count.
    pub fn depth(&self) -> usize {
        self.0.depth
    }
    /// Work estimate for traversal/comparison/retention.
    pub fn work_size(&self) -> usize {
        self.nodes()
    }
    /// Exact constant when the expression has already folded.
    pub fn as_constant(&self) -> Option<U256> {
        if let ExprKind::Constant(v) = self.0.kind {
            Some(v)
        } else {
            None
        }
    }
    /// Borrow the expression structure without exposing mutable graph nodes.
    pub fn kind(&self) -> &ExprKind {
        &self.0.kind
    }
    /// Immutable input leaves referenced by this expression.
    pub fn inputs(&self) -> BTreeSet<InputAtom> {
        let mut out = BTreeSet::new();
        self.visit(&mut |expr| {
            if let ExprKind::Input(input) = expr.kind() {
                out.insert(*input);
            }
        });
        out
    }
    /// Recover a word only from all 32 ordered BYTE(index, same_word) values.
    /// Partial, permuted, constant-folded or cross-scope bytes cannot establish
    /// this identity and must use ordinary conservative assembly instead.
    pub fn reassemble_word(bytes: &[Self]) -> Option<Self> {
        if bytes.len() != 32 {
            return None;
        }
        let mut original: Option<Self> = None;
        for (index, byte) in bytes.iter().enumerate() {
            let ExprKind::Operation { opcode: op, args } = byte.kind() else {
                return None;
            };
            if *op != opcode::BYTE || args[0].as_constant() != Some(U256::from(index)) {
                return None;
            }
            if original.as_ref().is_some_and(|word| word != &args[1]) {
                return None;
            }
            original.get_or_insert_with(|| args[1].clone());
        }
        original
    }
    /// Scoped input and runtime variable leaves referenced by this expression.
    pub fn variables(&self) -> BTreeSet<Self> {
        let mut out = BTreeSet::new();
        self.visit(&mut |expr| {
            if matches!(expr.kind(), ExprKind::Input(_) | ExprKind::Fresh(_)) {
                out.insert(expr.clone());
            }
        });
        out
    }
    /// Fresh runtime leaves requiring renaming when replaying an independent call.
    pub fn fresh_leaves(&self) -> BTreeSet<Self> {
        let mut out = BTreeSet::new();
        self.visit(&mut |expr| {
            if matches!(expr.kind(), ExprKind::Fresh(_)) {
                out.insert(expr.clone());
            }
        });
        out
    }
    fn visit(&self, action: &mut impl FnMut(&Self)) {
        walk::visit(self, action);
    }
    /// Rename only explicitly mapped fresh leaves; immutable inputs stay scoped.
    pub fn rename_fresh(
        &self,
        map: &BTreeMap<Self, Self>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        walk::Renamer::new(map, limits).expression(self)
    }
}

impl PartialEq for ExprId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.fingerprint == other.0.fingerprint && self.kind() == other.kind())
    }
}
impl Eq for ExprId {}
impl PartialOrd for ExprId {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for ExprId {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        if Arc::ptr_eq(&self.0, &other.0) {
            return CmpOrdering::Equal;
        }
        self.0
            .fingerprint
            .cmp(&other.0.fingerprint)
            .then_with(|| self.kind().cmp(other.kind()))
    }
}
impl Hash for ExprId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.fingerprint.hash(state);
    }
}

pub(crate) fn arity(op: u8) -> Option<usize> {
    match op {
        opcode::ISZERO | opcode::NOT | opcode::CLZ => Some(1),
        opcode::ADDMOD | opcode::MULMOD => Some(3),
        opcode::ADD
        | opcode::MUL
        | opcode::SUB
        | opcode::DIV
        | opcode::SDIV
        | opcode::MOD
        | opcode::SMOD
        | opcode::EXP
        | opcode::SIGNEXTEND
        | opcode::LT
        | opcode::GT
        | opcode::SLT
        | opcode::SGT
        | opcode::EQ
        | opcode::AND
        | opcode::OR
        | opcode::XOR
        | opcode::BYTE
        | opcode::SHL
        | opcode::SHR
        | opcode::SAR => Some(2),
        _ => None,
    }
}

impl Serialize for ExprId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SymbolicGraph::expression(self, GraphLimits::default())
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
