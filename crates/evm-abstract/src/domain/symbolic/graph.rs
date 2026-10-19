//! Bounded flat export of mixed word, byte and storage graphs.
//!
//! Edges use graph-local indices. Separate graphs in one native report bind
//! common leaves through an opaque process namespace and the original Fresh
//! token or input scope/symbol. Source namespaces still distinguish roles.
//! Different identities do not imply different values, contents or hashes.
//! These keys are not semantic equality evidence across reports or processes.
//! No recursive Arc serialization is used.

use super::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprLimits, InputAtom, SlotStateId,
    SourceId, StateBinding,
    structure::{Borrowed, Handle as Key, Mapper, Node, Postorder, Representation},
};
use crate::domain::NumericValue;
use alloy_primitives::U256;
use serde::Serialize;
use std::{
    collections::{BTreeMap, hash_map::RandomState},
    hash::BuildHasher,
    sync::{Arc, OnceLock},
};

#[cfg(test)]
mod tests;

fn identity_scope() -> &'static str {
    static SCOPE: OnceLock<String> = OnceLock::new();
    SCOPE.get_or_init(|| {
        // Randomized std hash seeds separate producer processes whose private
        // counters restart at one. This is an opaque label, not a global ID.
        let seed = (std::process::id(), std::time::SystemTime::now());
        let first = RandomState::new().hash_one(seed);
        let second = RandomState::new().hash_one(seed);
        format!("{first:016x}{second:016x}")
    })
}

/// Export resource policy, separate from logical byte ranges and execution work.
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
/// Resolve `token` before comparing sources across graphs in one report, then
/// include `namespace`; graph-local token indices alone cannot identify sources.
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
/// Typed references and native payloads in a flat exported graph.
#[derive(Clone, Debug)]
pub struct GraphReferences;
impl Representation for GraphReferences {
    type Word = usize;
    type Words = Vec<usize>;
    type Bindings = Vec<GraphBinding>;
    type Bytes = usize;
    type Slots = usize;
    type View = usize;
    type Constant = U256;
    type Input = InputAtom;
    type Exact = Arc<[u8]>;
    type Source = GraphSource;
    type Binding = GraphBinding;
    fn source_words(source: &GraphSource, action: &mut impl FnMut(usize)) {
        action(source.token);
    }
    fn binding_words(binding: &GraphBinding, action: &mut impl FnMut(usize)) {
        action(binding.value);
    }
}
/// Immutable exported node; every child is an earlier index in this graph.
pub type SymbolicGraphNode = Node<GraphReferences>;
/// Flat mixed state/expression graph, suitable for bounded native serialization.
///
/// Within one report, match leaves by `(identity_scope, Fresh token)` or
/// `(identity_scope, Input scope, symbol)`. Every graph exported by this process
/// shares the same opaque scope, generated once from standard-library randomized
/// hash seeds and process metadata. It is not a persistent or globally unique
/// identity, and consumers must not infer semantic equality across reports.
/// Node/depth and exact byte-payload limits remain local to each graph.
#[derive(Clone, Debug, Serialize)]
pub struct SymbolicGraph {
    /// Opaque producer namespace for leaf bindings inside one native report.
    pub identity_scope: String,
    /// Graph-local root index.
    pub root: usize,
    /// Nodes in dependency order.
    pub nodes: Vec<SymbolicGraphNode>,
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
        self.child(Key::Word(e.clone()))
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
    fn node(&mut self, key: &Key) -> SymbolicGraphNode {
        key.structure()
            .try_map(self)
            .unwrap_or_else(|never| match never {})
    }
    fn run(mut self, root: Key) -> Result<SymbolicGraph, ExprError> {
        let mut stack = Postorder::new(root.clone());
        while let Some((key, ready)) = stack.next() {
            if self.memo.contains_key(&key) {
                continue;
            }
            if !ready {
                self.work = self.work.saturating_add(1);
                let (nodes, depth) = (key.nodes(), key.depth());
                super::state::check(
                    nodes.max(self.work),
                    depth,
                    super::StateLimits {
                        max_nodes: self.limits.max_nodes,
                        max_depth: self.limits.max_depth,
                    },
                )?;
                let children = key.dependencies();
                stack.expand(key, children);
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
            identity_scope: identity_scope().to_owned(),
            root: self.child(root),
            nodes: self.nodes,
        })
    }
}
impl<'a> Mapper<Borrowed<'a>> for Export {
    type Output = GraphReferences;
    type Error = std::convert::Infallible;
    fn word(&mut self, value: &&ExprId) -> Result<usize, Self::Error> {
        Ok(self.e(value))
    }
    fn bytes(&mut self, value: &&ByteStateId) -> Result<usize, Self::Error> {
        Ok(self.b(value))
    }
    fn slots(&mut self, value: &&SlotStateId) -> Result<usize, Self::Error> {
        Ok(self.s(value))
    }
    fn view(&mut self, value: &&ByteView) -> Result<usize, Self::Error> {
        Ok(self.v(value))
    }
    fn constant(&mut self, value: &&U256) -> Result<U256, Self::Error> {
        Ok(**value)
    }
    fn input(&mut self, value: &&InputAtom) -> Result<InputAtom, Self::Error> {
        Ok(**value)
    }
    fn fresh(&mut self, value: u64) -> Result<u64, Self::Error> {
        Ok(value)
    }
    fn exact(&mut self, value: &&Arc<[u8]>) -> Result<Arc<[u8]>, Self::Error> {
        Ok(Arc::clone(value))
    }
    fn source(&mut self, value: &&SourceId) -> Result<GraphSource, Self::Error> {
        Ok(GraphSource {
            token: self.e(value.token()),
            namespace: value.namespace(),
        })
    }
    fn binding(&mut self, value: &&StateBinding) -> Result<GraphBinding, Self::Error> {
        Ok(GraphBinding {
            index: value.index,
            value: self.e(&value.value),
            numeric: value.numeric.clone(),
        })
    }
}
impl SymbolicGraph {
    /// Export an expression and every immutable state/source dependency.
    pub fn expression(root: &ExprId, limits: impl Into<GraphLimits>) -> Result<Self, ExprError> {
        Export::new(limits.into()).run(Key::Word(root.clone()))
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
