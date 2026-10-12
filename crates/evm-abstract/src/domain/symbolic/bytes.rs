//! Immutable byte states and bounded, zero-padded views.
//!
//! Copy retains the source snapshot and one range node even for a symbolic or
//! very large length. Reads apply mathematical index checks before BV arithmetic.

use super::{
    ExprError, ExprId, ExprLimits, SourceId, StateBinding, StateError, StateLimits,
    state::{self, Interner, StateNode},
};
use alloy_primitives::U256;
use serde::{Serialize, Serializer};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, OnceLock},
};

mod join;
mod reads;

/// Read-only byte state node. Dependencies always reference earlier snapshots.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum ByteStateKind {
    /// Unbounded zero array, used by fresh memory.
    Zero,
    /// Immutable source array; reads at or past length are zero.
    Input {
        /// Immutable source identity or captured bounded view.
        source: SourceId,
        /// Logical length, independent of materialized storage.
        length: ExprId,
        /// Initial observed cells, preserving their symbolic and numeric facts.
        bindings: Vec<StateBinding>,
    },
    /// Exact finite bytes with zero padding.
    Exact(Arc<[u8]>),
    /// Low eight bits written to one address.
    WriteByte {
        /// Previous immutable snapshot, before this write.
        previous: ByteStateId,
        /// First byte address, with nonwrapping read semantics.
        offset: ExprId,
        /// Word expression for the written or observed value.
        value: ExprId,
    },
    /// Big-endian 32-byte write, without address wraparound.
    WriteWord {
        /// Previous immutable snapshot, before this write.
        previous: ByteStateId,
        /// First byte address, with nonwrapping read semantics.
        offset: ExprId,
        /// Word expression for the written or observed value.
        value: ExprId,
    },
    /// Ordered range update; source is captured before this update.
    Copy {
        /// Previous immutable snapshot, before this write.
        previous: ByteStateId,
        /// First destination byte address.
        destination: ExprId,
        /// Immutable source identity or captured bounded view.
        source: ByteView,
        /// Logical length, independent of materialized storage.
        length: ExprId,
    },
    /// Zero-based array for a bounded source view, preserving its padding.
    Slice {
        /// Captured bounded source view.
        source: ByteView,
    },
    /// Paired branch alternatives, selected by nonzero=true.
    Ite {
        /// Nonzero EVM condition selecting the first alternative.
        condition: ExprId,
        /// Immutable state chosen when the condition is nonzero.
        then_state: ByteStateId,
        /// Immutable state chosen when the condition is zero.
        else_state: ByteStateId,
    },
}

/// Cheap immutable state handle. Equality uses collision-checked interning.
#[derive(Clone)]
pub struct ByteStateId(pub(super) Arc<StateNode<ByteStateKind>>);
static INTERN: OnceLock<Mutex<Interner<ByteStateKind>>> = OnceLock::new();
impl ByteStateId {
    fn intern(kind: ByteStateKind, nodes: usize, depth: usize) -> Self {
        Self(
            INTERN
                .get_or_init(|| Mutex::new(Interner::default()))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .intern(kind, nodes, depth),
        )
    }
    fn build(
        kind: ByteStateKind,
        children: impl IntoIterator<Item = (usize, usize)>,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        let (nodes, depth) = state::metrics(children);
        state::check(nodes, depth, limits)?;
        Ok(Self::intern(kind, nodes, depth))
    }
    /// Unbounded all-zero byte content; MSIZE is maintained by the world layer.
    pub fn zero() -> Self {
        Self::intern(ByteStateKind::Zero, 1, 1)
    }
    /// Immutable, independently identified finite source.
    pub fn input(source: SourceId, length: ExprId) -> Self {
        let (nodes, depth) = state::metrics([
            (source.token().nodes(), source.token().depth()),
            (length.nodes(), length.depth()),
        ]);
        Self::intern(
            ByteStateKind::Input {
                source,
                length,
                bindings: Vec::new(),
            },
            nodes,
            depth,
        )
    }
    /// Exact finite bytes. Shares the existing allocation; encoding is budgeted.
    pub fn exact(bytes: Arc<[u8]>) -> Self {
        Self::intern(ByteStateKind::Exact(bytes), 1, 1)
    }
    /// Initial observations bind the source array without overwriting unknown cells.
    pub fn observed(
        source: SourceId,
        length: ExprId,
        mut bindings: Vec<StateBinding>,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        bindings.sort_by_key(|binding| binding.index);
        let children = [
            (source.token().nodes(), source.token().depth()),
            (length.nodes(), length.depth()),
        ]
        .into_iter()
        .chain(bindings.iter().map(|b| {
            (
                b.value.nodes().saturating_add(b.numeric.work_size()),
                b.value.depth(),
            )
        }));
        Self::build(
            ByteStateKind::Input {
                source: source.clone(),
                length: length.clone(),
                bindings: bindings.clone(),
            },
            children,
            limits,
        )
    }
    /// Low eight bits are written, preserving all earlier reads.
    pub fn write_byte(
        &self,
        at: ExprId,
        value: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = self.kind()
        {
            let yes = then_state.write_byte(at.clone(), value.clone(), limits)?;
            let no = else_state.write_byte(at, value, limits)?;
            return yes.join_guarded(&no, condition.clone(), limits);
        }
        let previous = match self.kind() {
            ByteStateKind::WriteByte {
                previous, offset, ..
            } if *offset == at => previous,
            _ => self,
        };
        if self.unchanged_write(&at, &value, 1, limits)? {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        if matches!(previous.kind(), ByteStateKind::Zero)
            && value
                .as_constant()
                .is_some_and(|v| v & U256::from(255) == U256::ZERO)
        {
            state::check(previous.nodes(), previous.depth(), limits)?;
            return Ok(previous.clone());
        }
        Self::build(
            ByteStateKind::WriteByte {
                previous: previous.clone(),
                offset: at.clone(),
                value: value.clone(),
            },
            [
                (previous.nodes(), previous.depth()),
                (at.nodes(), at.depth()),
                (value.nodes(), value.depth()),
            ],
            limits,
        )
    }
    /// Big-endian word write. A 32-byte write is one retained state node.
    pub fn write_word(
        &self,
        at: ExprId,
        value: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = self.kind()
        {
            let yes = then_state.write_word(at.clone(), value.clone(), limits)?;
            let no = else_state.write_word(at, value, limits)?;
            return yes.join_guarded(&no, condition.clone(), limits);
        }
        // A complete overwrite discards older writes in the covered region.
        // Only structural or checked concrete coverage is used here; a modular
        // p+31 expression does not establish coverage when p might overflow.
        let mut previous = self;
        loop {
            match previous.kind() {
                ByteStateKind::WriteWord {
                    previous: old,
                    offset,
                    ..
                } if *offset == at => previous = old,
                ByteStateKind::WriteByte {
                    previous: old,
                    offset,
                    ..
                } if *offset == at
                    || matches!((at.as_constant(), offset.as_constant()), (Some(start), Some(byte)) if byte >= start && byte-start < U256::from(32)) =>
                {
                    previous = old
                }
                _ => break,
            }
        }
        if self.unchanged_write(&at, &value, 32, limits)? {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        if matches!(previous.kind(), ByteStateKind::Zero) && value.as_constant() == Some(U256::ZERO)
        {
            state::check(previous.nodes(), previous.depth(), limits)?;
            return Ok(previous.clone());
        }
        Self::build(
            ByteStateKind::WriteWord {
                previous: previous.clone(),
                offset: at.clone(),
                value: value.clone(),
            },
            [
                (previous.nodes(), previous.depth()),
                (at.nodes(), at.depth()),
                (value.nodes(), value.depth()),
            ],
            limits,
        )
    }
    // An identical write remains effective across disjoint writes. Clip the
    // visible intervals at MAX: address overflow never wraps to low memory.
    // Checking the retained graph before advancing bounds this iterative search
    // by the caller's existing nodes/depth policy and observes cancellation.
    fn unchanged_write(
        &self,
        at: &ExprId,
        value: &ExprId,
        width: u64,
        limits: StateLimits,
    ) -> Result<bool, StateError> {
        let mut cursor = self;
        loop {
            let (previous, offset, old, old_width) = match cursor.kind() {
                ByteStateKind::WriteByte {
                    previous,
                    offset,
                    value,
                } => (previous, offset, value, 1),
                ByteStateKind::WriteWord {
                    previous,
                    offset,
                    value,
                } => (previous, offset, value, 32),
                _ => return Ok(false),
            };
            if old_width == width && offset == at {
                return Ok(old == value);
            }
            let (Some(start), Some(old_start)) = (at.as_constant(), offset.as_constant()) else {
                return Ok(false);
            };
            let end = start.saturating_add(U256::from(width - 1));
            let old_end = old_start.saturating_add(U256::from(old_width - 1));
            if end >= old_start && old_end >= start {
                return Ok(false);
            }
            state::check(self.nodes(), self.depth(), limits)?;
            cursor = previous;
        }
    }
    /// Copy an arbitrary logical range without traversing it.
    pub fn copy(
        &self,
        dst: ExprId,
        source: ByteView,
        len: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if len.as_constant() == Some(U256::ZERO) {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        let source = source.canonicalize();
        if source.state == *self && source.offset == dst && source.length == len {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = self.kind()
        {
            let yes = then_state.copy(dst.clone(), source.clone(), len.clone(), limits)?;
            let no = else_state.copy(dst, source, len, limits)?;
            return yes.join_guarded(&no, condition.clone(), limits);
        }
        let previous = match self.kind() {
            ByteStateKind::Copy {
                previous,
                destination,
                source: old_source,
                length,
            } if *destination == dst && *length == len => {
                if *old_source == source {
                    state::check(self.nodes(), self.depth(), limits)?;
                    return Ok(self.clone());
                }
                previous
            }
            _ => self,
        };
        let children = [
            (previous.nodes(), previous.depth()),
            (dst.nodes(), dst.depth()),
            (source.nodes(), source.depth()),
            (len.nodes(), len.depth()),
        ];
        Self::build(
            ByteStateKind::Copy {
                previous: previous.clone(),
                destination: dst,
                source,
                length: len,
            },
            children,
            limits,
        )
    }
    /// Zero-based bounded subarray; later reads past len are zero.
    pub fn slice(&self, at: ExprId, len: ExprId, limits: StateLimits) -> Result<Self, StateError> {
        if len.as_constant() == Some(U256::ZERO) {
            state::check(1, 1, limits)?;
            return Ok(Self::zero());
        }
        let source = ByteView::new(self.clone(), at, len).canonicalize();
        if source.offset.as_constant() == Some(U256::ZERO) {
            match source.state.kind() {
                ByteStateKind::Input { length, .. } if *length == source.length => {
                    state::check(source.state.nodes(), source.state.depth(), limits)?;
                    return Ok(source.state);
                }
                ByteStateKind::Exact(bytes)
                    if source.length.as_constant() == Some(U256::from(bytes.len())) =>
                {
                    state::check(source.state.nodes(), source.state.depth(), limits)?;
                    return Ok(source.state);
                }
                ByteStateKind::Slice { source: inner } if inner.length == source.length => {
                    state::check(source.state.nodes(), source.state.depth(), limits)?;
                    return Ok(source.state);
                }
                ByteStateKind::Zero => {
                    state::check(source.state.nodes(), source.state.depth(), limits)?;
                    return Ok(source.state);
                }
                _ => {}
            }
        }
        let children = [(source.nodes(), source.depth())];
        Self::build(ByteStateKind::Slice { source }, children, limits)
    }
    /// Paired state join using the same guard as pointers and path conditions.
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
        if let Some(value) = selector.as_constant() {
            let selected = if value == U256::ZERO { other } else { self };
            state::check(selected.nodes(), selected.depth(), limits)?;
            return Ok(selected.clone());
        }
        state::check(self.nodes(), self.depth(), limits)?;
        state::check(other.nodes(), other.depth(), limits)?;
        state::check(selector.nodes(), selector.depth(), limits)?;
        if let Some(joined) = self.join_update(other, selector.clone(), limits)? {
            return Ok(joined);
        }
        Self::build(
            ByteStateKind::Ite {
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
    /// Read-only node kind for flat protocol export and state inspection.
    pub fn kind(&self) -> &ByteStateKind {
        &self.0.kind
    }
    /// Process-private graph identity; reports must remap this to a local index.
    pub fn id(&self) -> u64 {
        self.0.id
    }
    /// Conservative graph retention work; logical ranges are never counted as bytes.
    pub fn nodes(&self) -> usize {
        self.0.nodes
    }
    /// Dependency depth, including scalar offsets and guards.
    pub fn depth(&self) -> usize {
        self.0.depth
    }
    /// Retention/traversal estimate independent of logical range length.
    pub fn work_size(&self) -> usize {
        self.nodes()
    }
    /// All runtime symbols, including hidden source identities.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        super::walk::state_fresh(self)
    }
    /// Rename the entire immutable graph with shared memoization.
    pub fn rename_fresh(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        super::walk::Renamer::new(map, limits).bytes(self)
    }
}
impl PartialEq for ByteStateId {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}
impl Eq for ByteStateId {}
impl PartialOrd for ByteStateId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ByteStateId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id().cmp(&other.id())
    }
}
impl Hash for ByteStateId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}
impl fmt::Debug for ByteStateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ByteStateId {{ id: {}, nodes: {}, depth: {} }}",
            self.id(),
            self.nodes(),
            self.depth()
        )
    }
}
impl Serialize for ByteStateId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        super::SymbolicGraph::bytes(self, super::GraphLimits::default())
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
}

/// Bounded zero-padded content sequence, independent of its memory address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ByteView {
    /// Immutable backing state.
    pub state: ByteStateId,
    /// First backing byte, with mathematical rather than wrapping index semantics.
    pub offset: ExprId,
    /// Logical sequence length, including padding requested by the caller.
    pub length: ExprId,
}
impl ByteView {
    /// Capture a bounded view of an immutable state.
    pub fn new(state: ByteStateId, offset: ExprId, length: ExprId) -> Self {
        Self {
            state,
            offset,
            length,
        }
    }
    /// Preserve this view's boundary while making a further zero-padded slice.
    pub fn slice(&self, at: ExprId, len: ExprId, limits: StateLimits) -> Result<Self, StateError> {
        if at.as_constant() == Some(U256::ZERO) && len == self.length {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        let state = self
            .state
            .slice(self.offset.clone(), self.length.clone(), limits)?;
        state::check(at.nodes(), at.depth(), limits)?;
        state::check(len.nodes(), len.depth(), limits)?;
        let view = Self::new(state, at, len).canonicalize();
        state::check(view.nodes(), view.depth(), limits)?;
        Ok(view)
    }
    /// Recover complete-copy/slice identity without assuming arbitrary content equality.
    pub fn canonicalize(&self) -> Self {
        let mut view = self.clone();
        if view.length.as_constant() == Some(U256::ZERO) {
            return Self::new(
                ByteStateId::zero(),
                ExprId::constant(U256::ZERO),
                view.length,
            );
        }
        loop {
            let next = match view.state.kind() {
                ByteStateKind::Copy {
                    destination,
                    source,
                    length,
                    ..
                } if view.offset == *destination
                    && view.length == *length
                    && *length == source.length
                    && super::state::range_fits(destination, length) =>
                {
                    Some(source.clone())
                }
                ByteStateKind::Slice { source }
                    if view.offset.as_constant() == Some(U256::ZERO)
                        && view.length == source.length =>
                {
                    Some(source.clone())
                }
                _ => None,
            };
            match next {
                Some(next) => view = next,
                None => return view,
            }
        }
    }
    /// Conservative view and backing-state metadata size.
    pub fn nodes(&self) -> usize {
        self.state
            .nodes()
            .saturating_add(self.offset.nodes())
            .saturating_add(self.length.nodes())
    }
    /// Dependency depth before a consuming read/hash node.
    pub fn depth(&self) -> usize {
        self.state
            .depth()
            .max(self.offset.depth())
            .max(self.length.depth())
    }
    /// All runtime source, length, offset and state symbols.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        super::walk::view_fresh(self)
    }
    /// Rename view and backing-state symbols together.
    pub fn rename_fresh(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        super::walk::Renamer::new(map, limits).view(self)
    }
}
