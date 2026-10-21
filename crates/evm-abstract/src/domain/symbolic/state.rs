//! Shared state identities, resource bounds and weak structural interning.
//!
//! Hashes select an intern bucket only. Equality always compares the complete
//! node kind, so a hash collision cannot create an equality claim.

use super::{ExprError, ExprId, ExprLimits};
use crate::domain::{WordObservation, identity::Symbol};
use alloy_primitives::U256;
use serde::Serialize;
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use thiserror::Error;

pub(super) mod guarded;

/// Retention bounds on state metadata, independent of logical byte lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StateLimits {
    /// Conservative retained graph work, including scalar operands.
    pub max_nodes: usize,
    /// Maximum scalar/state dependency depth.
    pub max_depth: usize,
}
impl Default for StateLimits {
    fn default() -> Self {
        ExprLimits::default().into()
    }
}
impl From<ExprLimits> for StateLimits {
    fn from(value: ExprLimits) -> Self {
        Self {
            max_nodes: value.max_nodes,
            max_depth: value.max_depth,
        }
    }
}
impl From<StateLimits> for ExprLimits {
    fn from(value: StateLimits) -> Self {
        Self {
            max_nodes: value.max_nodes,
            max_depth: value.max_depth,
        }
    }
}

/// Failure to retain a complete state operation; the old snapshot remains valid.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StateError {
    /// The operation exceeds explicit state metadata bounds.
    #[error("state needs {nodes} nodes and depth {depth}, exceeding its policy")]
    /// Immutable state operation within its declared resource bounds.
    Limit {
        /// Retained graph work required.
        nodes: usize,
        /// Maximum scalar/state dependency depth.
        depth: usize,
    },
    /// Controlled analysis cancellation.
    #[error("state operation cancelled")]
    /// Immutable state operation within its declared resource bounds.
    Cancelled,
    /// Process-private identity namespace was exhausted.
    #[error("symbolic source identity exhausted")]
    /// Immutable state operation within its declared resource bounds.
    IdentityExhausted,
}
impl From<StateError> for ExprError {
    fn from(error: StateError) -> Self {
        match error {
            StateError::Cancelled => Self::Cancelled,
            StateError::Limit { nodes, depth } => Self::Limit { nodes, depth },
            StateError::IdentityExhausted => Self::Limit {
                nodes: usize::MAX,
                depth: usize::MAX,
            },
        }
    }
}
impl From<ExprError> for StateError {
    fn from(error: ExprError) -> Self {
        match error {
            ExprError::Cancelled => Self::Cancelled,
            ExprError::Limit { nodes, depth } => Self::Limit { nodes, depth },
            _ => Self::Limit {
                nodes: usize::MAX,
                depth: usize::MAX,
            },
        }
    }
}

/// Identity of an immutable byte/slot source, separate from its numeric length.
///
/// A snapshot can share a token across accounts with a distinct namespace for
/// each account/plane. Runtime sources require a fresh token. Tokens participate
/// in summary traversal and capture-avoiding renaming, even when not read as words.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SourceId {
    token: ExprId,
    namespace: U256,
}
impl SourceId {
    /// Scoped immutable source associated with an existing environment symbol.
    pub fn input(scope: u64, symbol: Symbol) -> Self {
        Self::scoped(ExprId::input(scope, symbol), U256::ZERO)
    }
    /// Independent runtime source; equal lengths do not merge identities.
    pub fn fresh() -> Result<Self, StateError> {
        ExprId::fresh()
            .map(|token| Self::scoped(token, U256::ZERO))
            .ok_or(StateError::IdentityExhausted)
    }
    /// Explicit source token and account/role namespace.
    pub fn scoped(token: ExprId, namespace: U256) -> Self {
        Self { token, namespace }
    }
    /// Source identity token, also visible to replay renaming.
    pub fn token(&self) -> &ExprId {
        &self.token
    }
    /// Caller-owned account/role namespace.
    pub fn namespace(&self) -> U256 {
        self.namespace
    }
    pub(super) fn renamed(&self, token: ExprId) -> Self {
        Self {
            token,
            namespace: self.namespace,
        }
    }
}

/// An observed initial array cell with both a symbolic binding and numeric facts.
/// Unknown unobserved cells stay unconstrained. This is not a state write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StateBinding {
    /// Concrete byte offset or slot key in the fixed snapshot.
    pub index: U256,
    /// Bound expression for the observed cell.
    pub value: ExprId,
    /// Sound scalar guarantees which constrain that same expression and cell.
    pub numeric: WordObservation,
}
impl Hash for StateBinding {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.value.hash(state);
        // Numeric facts are checked by full equality inside this hash bucket.
    }
}

pub(super) fn fingerprint(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
pub(super) fn check(nodes: usize, depth: usize, limits: StateLimits) -> Result<(), StateError> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(StateError::Cancelled);
    }
    if nodes == usize::MAX
        || depth == usize::MAX
        || nodes > limits.max_nodes
        || depth > limits.max_depth
    {
        return Err(StateError::Limit { nodes, depth });
    }
    Ok(())
}
pub(super) fn metrics(children: impl IntoIterator<Item = (usize, usize)>) -> (usize, usize) {
    children
        .into_iter()
        .fold((1usize, 1usize), |(nodes, depth), (n, d)| {
            (nodes.saturating_add(n), depth.max(d.saturating_add(1)))
        })
}

pub(super) struct StateNode<K> {
    pub id: u64,
    pub kind: K,
    pub nodes: usize,
    pub depth: usize,
}

// Byte and slot handles share interner identity and cached metadata, rather
// than ExprId's structural/fingerprint comparison. Keep this boundary explicit
// while sharing traversal, renaming and flat-export plumbing for both handles.
macro_rules! state_handle {
    ($handle:ident, $graph:ident, $fresh:ident) => {
        impl $handle {
            /// Process-private interner identity, remapped by each export.
            pub fn id(&self) -> u64 {
                self.0.id
            }
            /// Cached conservative metadata work, independent of logical ranges.
            pub fn nodes(&self) -> usize {
                self.0.nodes
            }
            /// Cached scalar/state dependency depth.
            pub fn depth(&self) -> usize {
                self.0.depth
            }
            /// Work estimate for graph retention and traversal.
            pub fn work_size(&self) -> usize {
                self.nodes()
            }
            /// Runtime leaves, including hidden source identities and guards.
            pub fn fresh_leaves(&self) -> std::collections::BTreeSet<super::ExprId> {
                super::walk::$fresh(self)
            }
            /// Capture-avoiding whole-graph rename with shared sort memoization.
            pub fn rename_fresh(
                &self,
                map: &std::collections::BTreeMap<super::ExprId, super::ExprId>,
                limits: super::ExprLimits,
            ) -> Result<Self, super::ExprError> {
                super::walk::Renamer::new(map, limits).$graph(self)
            }
        }
        impl PartialEq for $handle {
            fn eq(&self, other: &Self) -> bool {
                self.id() == other.id()
            }
        }
        impl Eq for $handle {}
        impl PartialOrd for $handle {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for $handle {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.id().cmp(&other.id())
            }
        }
        impl std::hash::Hash for $handle {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                std::hash::Hash::hash(&self.id(), state);
            }
        }
        impl std::fmt::Debug for $handle {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(
                    f,
                    concat!(stringify!($handle), " {{ id: {}, nodes: {}, depth: {} }}"),
                    self.id(),
                    self.nodes(),
                    self.depth()
                )
            }
        }
        impl serde::Serialize for $handle {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(
                    &super::SymbolicGraph::$graph(self, super::GraphLimits::default())
                        .map_err(serde::ser::Error::custom)?,
                    serializer,
                )
            }
        }
    };
}
pub(super) use state_handle;
static NEXT_NODE: AtomicU64 = AtomicU64::new(1);

/// Weak caches do not retain previous snapshots or source expressions.
/// Live identical kinds always reuse the same id; dead entries are reclaimed.
pub(super) struct Interner<K> {
    buckets: HashMap<u64, Vec<Weak<StateNode<K>>>>,
    insertions: usize,
}
impl<K> Default for Interner<K> {
    fn default() -> Self {
        Self {
            buckets: HashMap::new(),
            insertions: 0,
        }
    }
}
impl<K: Eq + Hash> Interner<K> {
    pub fn intern(&mut self, kind: K, nodes: usize, depth: usize) -> Arc<StateNode<K>> {
        let bucket = self.buckets.entry(fingerprint(&kind)).or_default();
        let mut found = None;
        bucket.retain(|weak| {
            if let Some(node) = weak.upgrade() {
                if node.kind == kind {
                    found = Some(node);
                }
                true
            } else {
                false
            }
        });
        if let Some(node) = found {
            return node;
        }
        let id = NEXT_NODE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("state node identity exhausted");
        let node = Arc::new(StateNode {
            id,
            kind,
            nodes,
            depth,
        });
        bucket.push(Arc::downgrade(&node));
        self.insertions += 1;
        if self.insertions.is_multiple_of(256) {
            self.buckets.retain(|_, bucket| {
                bucket.retain(|weak| weak.strong_count() > 0);
                !bucket.is_empty()
            });
        }
        node
    }
}

/// Whether all indices in a non-wrapping Word256 range are structurally valid.
/// Symbolic lengths at nonzero destinations require a solver guard instead.
pub(super) fn range_fits(offset: &ExprId, length: &ExprId) -> bool {
    if offset.as_constant() == Some(U256::ZERO) || length.as_constant() == Some(U256::ZERO) {
        return true;
    }
    match (offset.as_constant(), length.as_constant()) {
        (Some(offset), Some(length)) => offset.checked_add(length - U256::from(1)).is_some(),
        _ => false,
    }
}
