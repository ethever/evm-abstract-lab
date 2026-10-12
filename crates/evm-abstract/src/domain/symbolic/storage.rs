//! Immutable ordered Word256 storage states, shared across checkpoints.

use super::{
    ExprError, ExprId, ExprLimits, SourceId, StateBinding, StateError, StateLimits,
    state::{self, Interner, StateNode},
};
use serde::{Serialize, Serializer};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, OnceLock},
};

/// Read-only storage node; account and plane identity live in its source.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum SlotStateKind {
    /// Known initial all-zero storage, including a new transient plane.
    Zero,
    /// Fixed snapshot source with optional observed initial cells.
    Input {
        /// Immutable source identity or captured bounded view.
        source: SourceId,
        /// Initial observed cells, preserving their symbolic and numeric facts.
        bindings: Vec<StateBinding>,
    },
    /// One ordered write, for both concrete and symbolic keys.
    Store {
        /// Previous immutable snapshot, before this write.
        previous: SlotStateId,
        /// Word256 storage key.
        key: ExprId,
        /// Word expression for the written or observed value.
        value: ExprId,
    },
    /// Paired branch alternatives, selected by nonzero=true.
    Ite {
        /// Nonzero EVM condition selecting the first alternative.
        condition: ExprId,
        /// Immutable state chosen when the condition is nonzero.
        then_state: SlotStateId,
        /// Immutable state chosen when the condition is zero.
        else_state: SlotStateId,
    },
}
/// Immutable, structurally interned storage state handle.
#[derive(Clone)]
pub struct SlotStateId(pub(super) Arc<StateNode<SlotStateKind>>);
static INTERN: OnceLock<Mutex<Interner<SlotStateKind>>> = OnceLock::new();
impl SlotStateId {
    fn intern(kind: SlotStateKind, nodes: usize, depth: usize) -> Self {
        Self(
            INTERN
                .get_or_init(|| Mutex::new(Interner::default()))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .intern(kind, nodes, depth),
        )
    }
    fn build(
        kind: SlotStateKind,
        children: impl IntoIterator<Item = (usize, usize)>,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        let (nodes, depth) = state::metrics(children);
        state::check(nodes, depth, limits)?;
        Ok(Self::intern(kind, nodes, depth))
    }
    /// Known all-zero initial storage.
    pub fn zero() -> Self {
        Self::intern(SlotStateKind::Zero, 1, 1)
    }
    /// Unobserved snapshot cells are unconstrained and stable within this source.
    pub fn input(source: SourceId) -> Self {
        let (nodes, depth) = state::metrics([(source.token().nodes(), source.token().depth())]);
        Self::intern(
            SlotStateKind::Input {
                source,
                bindings: Vec::new(),
            },
            nodes,
            depth,
        )
    }
    /// Bind observed initial cells to both symbolic and numeric information.
    pub fn observed(
        source: SourceId,
        mut bindings: Vec<StateBinding>,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        bindings.sort_by_key(|b| b.index);
        let children = std::iter::once((source.token().nodes(), source.token().depth())).chain(
            bindings.iter().map(|b| {
                (
                    b.value.nodes().saturating_add(b.numeric.work_size()),
                    b.value.depth(),
                )
            }),
        );
        Self::build(
            SlotStateKind::Input {
                source: source.clone(),
                bindings: bindings.clone(),
            },
            children,
            limits,
        )
    }
    /// Strong ordered update. Repeated identical keys overwrite earlier nodes.
    pub fn store(
        &self,
        key: ExprId,
        value: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if let SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = self.kind()
        {
            let yes = then_state.store(key.clone(), value.clone(), limits)?;
            let no = else_state.store(key, value, limits)?;
            return yes.join_guarded(&no, condition.clone(), limits);
        }
        let previous = match self.kind() {
            SlotStateKind::Store {
                previous, key: old, ..
            } if *old == key => previous,
            _ => self,
        };
        // Only unequal concrete keys prove disjointness. Different symbolic
        // expressions can still alias, so they stop the search and retain order.
        let mut cursor = self;
        while let SlotStateKind::Store {
            previous,
            key: old,
            value: old_value,
        } = cursor.kind()
        {
            if *old == key && *old_value == value {
                state::check(self.nodes(), self.depth(), limits)?;
                return Ok(self.clone());
            }
            if !matches!((old.as_constant(), key.as_constant()), (Some(a), Some(b)) if a != b) {
                break;
            }
            // The checked graph bounds the write spine and cancellation remains
            // observable even when this update ultimately returns the old root.
            state::check(self.nodes(), self.depth(), limits)?;
            cursor = previous;
        }
        if matches!(previous.kind(), SlotStateKind::Zero)
            && value.as_constant() == Some(alloy_primitives::U256::ZERO)
        {
            state::check(previous.nodes(), previous.depth(), limits)?;
            return Ok(previous.clone());
        }
        Self::build(
            SlotStateKind::Store {
                previous: previous.clone(),
                key: key.clone(),
                value: value.clone(),
            },
            [
                (previous.nodes(), previous.depth()),
                (key.nodes(), key.depth()),
                (value.nodes(), value.depth()),
            ],
            limits,
        )
    }
    /// Join alternatives with the same nonzero guard as the machine payload.
    pub fn join_guarded(
        &self,
        other: &Self,
        selector: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if self == other {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        if let Some(v) = selector.as_constant() {
            let s = if v == alloy_primitives::U256::ZERO {
                other
            } else {
                self
            };
            state::check(s.nodes(), s.depth(), limits)?;
            return Ok(s.clone());
        }
        Self::build(
            SlotStateKind::Ite {
                condition: selector.clone(),
                then_state: self.clone(),
                else_state: other.clone(),
            },
            [
                (self.nodes(), self.depth()),
                (other.nodes(), other.depth()),
                (selector.nodes(), selector.depth()),
            ],
            limits,
        )
    }
    /// Immutable node shape for protocol export and inspection.
    pub fn kind(&self) -> &SlotStateKind {
        &self.0.kind
    }
    /// Private graph id, remapped by each report.
    pub fn id(&self) -> u64 {
        self.0.id
    }
    /// Conservative retained metadata work.
    pub fn nodes(&self) -> usize {
        self.0.nodes
    }
    /// Maximum dependency depth.
    pub fn depth(&self) -> usize {
        self.0.depth
    }
    /// Work estimate for graph operations.
    pub fn work_size(&self) -> usize {
        self.nodes()
    }
    /// Runtime identities from keys, values, guards and source tokens.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        super::walk::slots_fresh(self)
    }
    /// Capture-avoiding whole-graph rename.
    pub fn rename_fresh(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        super::walk::Renamer::new(map, limits).slots(self)
    }
}
impl PartialEq for SlotStateId {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}
impl Eq for SlotStateId {}
impl PartialOrd for SlotStateId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SlotStateId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id().cmp(&other.id())
    }
}
impl Hash for SlotStateId {
    fn hash<H: Hasher>(&self, s: &mut H) {
        self.id().hash(s);
    }
}
impl fmt::Debug for SlotStateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlotStateId")
            .field("id", &self.id())
            .field("nodes", &self.nodes())
            .field("depth", &self.depth())
            .finish()
    }
}
impl Serialize for SlotStateId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        super::SymbolicGraph::slots(self, super::GraphLimits::default())
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}
