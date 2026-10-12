//! Bounded flat export of mixed word, byte and storage graphs.
//! Reports assign their own indices; private source/node identities never serve
//! as cross-report equality evidence. No recursive Arc serialization is used.

use super::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, ExprLimits, InputAtom,
    SlotStateId, SlotStateKind, SourceId, StateBinding,
};
use crate::domain::NumericValue;
use alloy_primitives::U256;
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

/// Export resource policy, separate from logical byte ranges and solver work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GraphLimits {
    /// Maximum graph metadata/scalar traversal work.
    pub max_nodes: usize,
    /// Maximum graph dependency depth.
    pub max_depth: usize,
    /// Maximum exact byte payload retained by one complete export.
    pub max_exact_bytes: usize,
}
impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_nodes: 65_536,
            max_depth: 256,
            max_exact_bytes: 16 * 1024 * 1024,
        }
    }
}
impl From<ExprLimits> for GraphLimits {
    fn from(limits: ExprLimits) -> Self {
        Self {
            max_nodes: limits.max_nodes,
            max_depth: limits.max_depth,
            ..Self::default()
        }
    }
}

/// A source reference in the same flat graph as its scalar identity token.
#[derive(Clone, Debug, Serialize)]
pub struct GraphSource {
    /// Graph-local expression index.
    pub token: usize,
    /// Account/plane or source role namespace.
    pub namespace: U256,
}
/// Observed initial cell, preserving the same expression and numeric facts.
#[derive(Clone, Debug, Serialize)]
pub struct GraphBinding {
    /// Fixed initial cell index.
    pub index: U256,
    /// Graph-local expression index.
    pub value: usize,
    /// Observed numeric guarantees.
    pub numeric: NumericValue,
}
/// Immutable exported node; every child is an earlier index in this graph.
#[derive(Clone, Debug, Serialize)]
pub enum SymbolicGraphNode {
    /// An exact Word256 literal.
    Constant(U256),
    /// A scoped immutable environment input.
    Input(InputAtom),
    /// A graph-local independent runtime identity.
    Fresh,
    /// A pure EVM word operation in pop-argument order.
    Operation {
        /// Pure EVM opcode.
        opcode: u8,
        /// Operand references in EVM pop order.
        args: Vec<usize>,
    },
    /// A nonzero condition selects the corresponding word.
    Ite {
        /// Nonzero EVM condition selecting the first alternative.
        condition: usize,
        /// Word chosen when the condition is nonzero.
        then_value: usize,
        /// Word chosen when the condition is zero.
        else_value: usize,
    },
    /// Read a byte and zero extend it to Word256.
    ByteRead {
        /// Immutable backing-state reference.
        state: usize,
        /// Byte address or fixed observed cell key.
        index: usize,
    },
    /// Read 32 big-endian bytes from a bounded view.
    WordRead {
        /// Bounded, zero-padded source sequence.
        view: usize,
        /// First byte address, with nonwrapping read semantics.
        offset: usize,
    },
    /// Read a Word256 slot from an immutable storage state.
    SlotRead {
        /// Immutable backing-state reference.
        state: usize,
        /// Word256 storage key.
        key: usize,
    },
    /// Hash the complete bounded content sequence.
    Keccak {
        /// Complete bounded byte sequence passed to Keccak.
        input: usize,
    },
    /// An unbounded all-zero byte state.
    BytesZero,
    /// An immutable finite source with observed initial bytes.
    BytesInput {
        /// Immutable source identity or captured bounded view.
        source: GraphSource,
        /// Logical length, independent of materialized storage.
        length: usize,
        /// Initial observed cells, preserving their symbolic and numeric facts.
        bindings: Vec<GraphBinding>,
    },
    /// A concrete finite byte sequence, padded with zeros.
    BytesExact(Arc<[u8]>),
    /// An ordered low-eight-bit byte update.
    WriteByte {
        /// Previous immutable snapshot, before this write.
        previous: usize,
        /// First byte address, with nonwrapping read semantics.
        offset: usize,
        /// Word expression for the written or observed value.
        value: usize,
    },
    /// An ordered big-endian 32-byte update.
    WriteWord {
        /// Previous immutable snapshot, before this write.
        previous: usize,
        /// First byte address, with nonwrapping read semantics.
        offset: usize,
        /// Word expression for the written or observed value.
        value: usize,
    },
    /// A lazy ordered range copy from a captured view.
    Copy {
        /// Previous immutable snapshot, before this write.
        previous: usize,
        /// First destination byte address.
        destination: usize,
        /// Immutable source identity or captured bounded view.
        source: usize,
        /// Logical length, independent of materialized storage.
        length: usize,
    },
    /// A zero-based state preserving the source view boundary.
    Slice {
        /// Immutable source identity or captured bounded view.
        source: usize,
    },
    /// A nonzero condition selects the paired byte state.
    BytesIte {
        /// Nonzero EVM condition selecting the first alternative.
        condition: usize,
        /// Immutable state chosen when the condition is nonzero.
        then_state: usize,
        /// Immutable state chosen when the condition is zero.
        else_state: usize,
    },
    /// Known all-zero initial storage.
    SlotsZero,
    /// A fixed storage source with observed initial cells.
    SlotsInput {
        /// Immutable source identity or captured bounded view.
        source: GraphSource,
        /// Initial observed cells, preserving their symbolic and numeric facts.
        bindings: Vec<GraphBinding>,
    },
    /// An ordered Word256 slot write.
    Store {
        /// Previous immutable snapshot, before this write.
        previous: usize,
        /// Word256 storage key.
        key: usize,
        /// Word expression for the written or observed value.
        value: usize,
    },
    /// A nonzero condition selects the paired storage state.
    SlotsIte {
        /// Nonzero EVM condition selecting the first alternative.
        condition: usize,
        /// Immutable state chosen when the condition is nonzero.
        then_state: usize,
        /// Immutable state chosen when the condition is zero.
        else_state: usize,
    },
    /// A bounded, zero-padded sequence over a backing byte state.
    View {
        /// Immutable backing-state reference.
        state: usize,
        /// First byte address, with nonwrapping read semantics.
        offset: usize,
        /// Logical length, independent of materialized storage.
        length: usize,
    },
}
/// Flat mixed state/expression graph, suitable for bounded native serialization.
#[derive(Clone, Debug, Serialize)]
pub struct SymbolicGraph {
    /// Graph-local root index.
    pub root: usize,
    /// Nodes in dependency order.
    pub nodes: Vec<SymbolicGraphNode>,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Expr(ExprId),
    Bytes(ByteStateId),
    Slots(SlotStateId),
    View(ByteView),
}
struct Export {
    nodes: Vec<SymbolicGraphNode>,
    memo: BTreeMap<Key, usize>,
    limits: GraphLimits,
    work: usize,
    exact_bytes: usize,
}
impl Export {
    fn new(limits: GraphLimits) -> Self {
        Self {
            nodes: Vec::new(),
            memo: BTreeMap::new(),
            limits,
            work: 0,
            exact_bytes: 0,
        }
    }
    fn child(&self, key: Key) -> usize {
        *self.memo.get(&key).expect("postorder export")
    }
    fn e(&self, e: &ExprId) -> usize {
        self.child(Key::Expr(e.clone()))
    }
    fn b(&self, b: &ByteStateId) -> usize {
        self.child(Key::Bytes(b.clone()))
    }
    fn s(&self, s: &SlotStateId) -> usize {
        self.child(Key::Slots(s.clone()))
    }
    fn v(&self, v: &ByteView) -> usize {
        self.child(Key::View(v.clone()))
    }
    fn source(&self, s: &SourceId) -> GraphSource {
        GraphSource {
            token: self.e(s.token()),
            namespace: s.namespace(),
        }
    }
    fn bindings(&self, bindings: &[StateBinding]) -> Vec<GraphBinding> {
        bindings
            .iter()
            .map(|b| GraphBinding {
                index: b.index,
                value: self.e(&b.value),
                numeric: b.numeric.clone(),
            })
            .collect()
    }
    fn node(&self, key: &Key) -> SymbolicGraphNode {
        match key {
            Key::Expr(e) => match e.kind() {
                ExprKind::Constant(v) => SymbolicGraphNode::Constant(*v),
                ExprKind::Input(i) => SymbolicGraphNode::Input(*i),
                ExprKind::Fresh(_) => SymbolicGraphNode::Fresh,
                ExprKind::Operation { opcode, args } => SymbolicGraphNode::Operation {
                    opcode: *opcode,
                    args: args.iter().map(|a| self.e(a)).collect(),
                },
                ExprKind::Ite {
                    condition,
                    then_value,
                    else_value,
                } => SymbolicGraphNode::Ite {
                    condition: self.e(condition),
                    then_value: self.e(then_value),
                    else_value: self.e(else_value),
                },
                ExprKind::ByteRead { state, index } => SymbolicGraphNode::ByteRead {
                    state: self.b(state),
                    index: self.e(index),
                },
                ExprKind::WordRead { view, offset } => SymbolicGraphNode::WordRead {
                    view: self.v(view),
                    offset: self.e(offset),
                },
                ExprKind::SlotRead { state, key } => SymbolicGraphNode::SlotRead {
                    state: self.s(state),
                    key: self.e(key),
                },
                ExprKind::Keccak { input } => SymbolicGraphNode::Keccak {
                    input: self.v(input),
                },
            },
            Key::Bytes(b) => match b.kind() {
                ByteStateKind::Zero => SymbolicGraphNode::BytesZero,
                ByteStateKind::Exact(bytes) => SymbolicGraphNode::BytesExact(Arc::clone(bytes)),
                ByteStateKind::Input {
                    source,
                    length,
                    bindings,
                } => SymbolicGraphNode::BytesInput {
                    source: self.source(source),
                    length: self.e(length),
                    bindings: self.bindings(bindings),
                },
                ByteStateKind::WriteByte {
                    previous,
                    offset,
                    value,
                } => SymbolicGraphNode::WriteByte {
                    previous: self.b(previous),
                    offset: self.e(offset),
                    value: self.e(value),
                },
                ByteStateKind::WriteWord {
                    previous,
                    offset,
                    value,
                } => SymbolicGraphNode::WriteWord {
                    previous: self.b(previous),
                    offset: self.e(offset),
                    value: self.e(value),
                },
                ByteStateKind::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => SymbolicGraphNode::Copy {
                    previous: self.b(previous),
                    destination: self.e(destination),
                    source: self.v(source),
                    length: self.e(length),
                },
                ByteStateKind::Slice { source } => SymbolicGraphNode::Slice {
                    source: self.v(source),
                },
                ByteStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => SymbolicGraphNode::BytesIte {
                    condition: self.e(condition),
                    then_state: self.b(then_state),
                    else_state: self.b(else_state),
                },
            },
            Key::Slots(s) => match s.kind() {
                SlotStateKind::Zero => SymbolicGraphNode::SlotsZero,
                SlotStateKind::Input { source, bindings } => SymbolicGraphNode::SlotsInput {
                    source: self.source(source),
                    bindings: self.bindings(bindings),
                },
                SlotStateKind::Store {
                    previous,
                    key,
                    value,
                } => SymbolicGraphNode::Store {
                    previous: self.s(previous),
                    key: self.e(key),
                    value: self.e(value),
                },
                SlotStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => SymbolicGraphNode::SlotsIte {
                    condition: self.e(condition),
                    then_state: self.s(then_state),
                    else_state: self.s(else_state),
                },
            },
            Key::View(v) => SymbolicGraphNode::View {
                state: self.b(&v.state),
                offset: self.e(&v.offset),
                length: self.e(&v.length),
            },
        }
    }
    fn run(mut self, root: Key) -> Result<SymbolicGraph, ExprError> {
        let mut stack = vec![(root.clone(), false)];
        while let Some((key, ready)) = stack.pop() {
            if self.memo.contains_key(&key) {
                continue;
            }
            if !ready {
                self.work = self.work.saturating_add(1);
                let (nodes, depth) = match &key {
                    Key::Expr(e) => (e.nodes(), e.depth()),
                    Key::Bytes(b) => (b.nodes(), b.depth()),
                    Key::Slots(s) => (s.nodes(), s.depth()),
                    Key::View(v) => (v.nodes(), v.depth()),
                };
                super::state::check(
                    nodes.max(self.work),
                    depth,
                    super::StateLimits {
                        max_nodes: self.limits.max_nodes,
                        max_depth: self.limits.max_depth,
                    },
                )?;
                let children = dependencies(&key);
                stack.push((key, true));
                stack.extend(children.into_iter().rev().map(|c| (c, false)));
                continue;
            }
            if let Key::Bytes(b) = &key
                && let ByteStateKind::Exact(bytes) = b.kind()
            {
                self.exact_bytes = self.exact_bytes.saturating_add(bytes.len());
                if self.exact_bytes > self.limits.max_exact_bytes {
                    return Err(ExprError::ExportBytes {
                        bytes: self.exact_bytes,
                        limit: self.limits.max_exact_bytes,
                    });
                }
            }
            super::state::check(
                self.work,
                1,
                super::StateLimits {
                    max_nodes: self.limits.max_nodes,
                    max_depth: self.limits.max_depth,
                },
            )?;
            let node = self.node(&key);
            let index = self.nodes.len();
            self.nodes.push(node);
            self.memo.insert(key, index);
        }
        Ok(SymbolicGraph {
            root: self.child(root),
            nodes: self.nodes,
        })
    }
}
fn dependencies(key: &Key) -> Vec<Key> {
    let e = |e: &ExprId| Key::Expr(e.clone());
    match key {
        Key::Expr(expr) => match expr.kind() {
            ExprKind::Operation { args, .. } => args.iter().map(e).collect(),
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => vec![e(condition), e(then_value), e(else_value)],
            ExprKind::ByteRead { state, index } => vec![Key::Bytes(state.clone()), e(index)],
            ExprKind::WordRead { view, offset } => vec![Key::View(view.clone()), e(offset)],
            ExprKind::SlotRead { state, key } => vec![Key::Slots(state.clone()), e(key)],
            ExprKind::Keccak { input } => vec![Key::View(input.clone())],
            _ => Vec::new(),
        },
        Key::Bytes(b) => match b.kind() {
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => std::iter::once(e(source.token()))
                .chain(std::iter::once(e(length)))
                .chain(bindings.iter().map(|b| e(&b.value)))
                .collect(),
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            }
            | ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => vec![Key::Bytes(previous.clone()), e(offset), e(value)],
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => vec![
                Key::Bytes(previous.clone()),
                e(destination),
                Key::View(source.clone()),
                e(length),
            ],
            ByteStateKind::Slice { source } => vec![Key::View(source.clone())],
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => vec![
                e(condition),
                Key::Bytes(then_state.clone()),
                Key::Bytes(else_state.clone()),
            ],
            _ => Vec::new(),
        },
        Key::Slots(s) => match s.kind() {
            SlotStateKind::Input { source, bindings } => std::iter::once(e(source.token()))
                .chain(bindings.iter().map(|b| e(&b.value)))
                .collect(),
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => vec![Key::Slots(previous.clone()), e(key), e(value)],
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => vec![
                e(condition),
                Key::Slots(then_state.clone()),
                Key::Slots(else_state.clone()),
            ],
            _ => Vec::new(),
        },
        Key::View(v) => vec![Key::Bytes(v.state.clone()), e(&v.offset), e(&v.length)],
    }
}
impl SymbolicGraph {
    /// Export an expression and every immutable state/source dependency.
    pub fn expression(root: &ExprId, limits: impl Into<GraphLimits>) -> Result<Self, ExprError> {
        Export::new(limits.into()).run(Key::Expr(root.clone()))
    }
    /// Export a complete byte-state graph.
    pub fn bytes(root: &ByteStateId, limits: impl Into<GraphLimits>) -> Result<Self, ExprError> {
        Export::new(limits.into()).run(Key::Bytes(root.clone()))
    }
    /// Export a complete slot-state graph.
    pub fn slots(root: &SlotStateId, limits: impl Into<GraphLimits>) -> Result<Self, ExprError> {
        Export::new(limits.into()).run(Key::Slots(root.clone()))
    }
    /// Export a bounded view together with all content/length/offset dependencies.
    pub fn view(root: &ByteView, limits: impl Into<GraphLimits>) -> Result<Self, ExprError> {
        Export::new(limits.into()).run(Key::View(root.clone()))
    }
}
