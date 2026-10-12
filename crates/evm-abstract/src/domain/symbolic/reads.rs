//! Word expressions with immutable state semantics and bounded construction.

use super::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, ExprLimits, Node,
    SlotStateId, SlotStateKind, state,
};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;
use std::sync::Arc;

impl ExprId {
    pub(super) fn retained(
        kind: ExprKind,
        nodes: usize,
        depth: usize,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        state::check(nodes, depth, limits.into())?;
        let state_semantics = match &kind {
            ExprKind::ByteRead { .. }
            | ExprKind::WordRead { .. }
            | ExprKind::SlotRead { .. }
            | ExprKind::Keccak { .. } => true,
            ExprKind::Operation { args, .. } => args.iter().any(Self::has_state_semantics),
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => {
                condition.has_state_semantics()
                    || then_value.has_state_semantics()
                    || else_value.has_state_semantics()
            }
            _ => false,
        };
        let fingerprint = state::fingerprint(&kind);
        Ok(Self(Arc::new(Node {
            kind,
            nodes,
            depth,
            fingerprint,
            state_semantics,
        })))
    }
    /// An EVM nonzero guard chooses yes. Identical alternatives fold immediately.
    pub fn ite(
        condition: Self,
        yes: Self,
        no: Self,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        if yes == no {
            state::check(yes.nodes(), yes.depth(), limits.into())?;
            return Ok(yes);
        }
        if let Some(v) = condition.as_constant() {
            let out = if v == U256::ZERO { no } else { yes };
            state::check(out.nodes(), out.depth(), limits.into())?;
            return Ok(out);
        }
        let (nodes, depth) = state::metrics([
            (condition.nodes(), condition.depth()),
            (yes.nodes(), yes.depth()),
            (no.nodes(), no.depth()),
        ]);
        Self::retained(
            ExprKind::Ite {
                condition,
                then_value: yes,
                else_value: no,
            },
            nodes,
            depth,
            limits,
        )
    }
    /// Read one byte as a zero-extended Word256 value.
    pub fn byte_read(
        state: ByteStateId,
        index: Self,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        let (nodes, depth) = state::metrics([
            (state.nodes(), state.depth()),
            (index.nodes(), index.depth()),
        ]);
        state::check(nodes, depth, limits.into())?;
        match state.kind() {
            ByteStateKind::Zero => return Ok(Self::constant(U256::ZERO)),
            ByteStateKind::Exact(bytes) => {
                if let Some(i) = index.as_constant() {
                    let v = usize::try_from(i)
                        .ok()
                        .and_then(|i| bytes.get(i))
                        .copied()
                        .unwrap_or(0);
                    return Ok(Self::constant(U256::from(v)));
                }
            }
            ByteStateKind::WriteByte { offset, value, .. } if *offset == index => {
                return Self::operation(
                    opcode::AND,
                    &[value.clone(), Self::constant(U256::from(255))],
                    limits,
                );
            }
            _ => {}
        }
        Self::retained(ExprKind::ByteRead { state, index }, nodes, depth, limits)
    }
    /// Read 32 big-endian bytes, zero padding past this view's length.
    /// Relative/backing offset addition is checked mathematically in lowering.
    pub fn word_read(view: ByteView, at: Self, limits: ExprLimits) -> Result<Self, ExprError> {
        let view = view.canonicalize();
        let (nodes, depth) =
            state::metrics([(view.nodes(), view.depth()), (at.nodes(), at.depth())]);
        state::check(nodes, depth, limits.into())?;
        if view.length.as_constant() == Some(U256::ZERO)
            || matches!(view.state.kind(), ByteStateKind::Zero)
        {
            return Ok(Self::constant(U256::ZERO));
        }
        if let (Some(offset), Some(relative), Some(len)) = (
            view.offset.as_constant(),
            at.as_constant(),
            view.length.as_constant(),
        ) && let ByteStateKind::Exact(bytes) = view.state.kind()
        {
            let mut word = [0u8; 32];
            for (j, out) in word.iter_mut().enumerate() {
                if let Some(index) = relative
                    .checked_add(U256::from(j))
                    .filter(|i| *i < len)
                    .and_then(|i| offset.checked_add(i))
                    .and_then(|i| usize::try_from(i).ok())
                {
                    *out = bytes.get(index).copied().unwrap_or(0);
                }
            }
            return Ok(Self::constant(U256::from_be_bytes(word)));
        }

        if at.as_constant() == Some(U256::ZERO)
            && view
                .length
                .as_constant()
                .is_some_and(|l| l >= U256::from(32))
            && let ByteStateKind::WriteWord { offset, value, .. } = view.state.kind()
            && *offset == view.offset
            && offset
                .as_constant()
                .is_some_and(|o| o <= U256::MAX - U256::from(31))
        {
            state::check(value.nodes(), value.depth(), limits.into())?;
            return Ok(value.clone());
        }
        Self::retained(
            ExprKind::WordRead { view, offset: at },
            nodes,
            depth,
            limits,
        )
    }
    /// Ordered array read; a structurally identical latest key folds locally.
    pub fn slot_read(state: SlotStateId, key: Self, limits: ExprLimits) -> Result<Self, ExprError> {
        let (nodes, depth) =
            state::metrics([(state.nodes(), state.depth()), (key.nodes(), key.depth())]);
        state::check(nodes, depth, limits.into())?;
        match state.kind() {
            SlotStateKind::Zero => return Ok(Self::constant(U256::ZERO)),
            SlotStateKind::Store {
                key: stored, value, ..
            } if *stored == key => {
                state::check(value.nodes(), value.depth(), limits.into())?;
                return Ok(value.clone());
            }
            SlotStateKind::Store {
                previous,
                key: stored,
                ..
            } if matches!((stored.as_constant(), key.as_constant()), (Some(a), Some(b)) if a != b) =>
            {
                // Only unequal concrete keys prove nonaliasing. Keep the
                // previous immutable snapshot for this demanded fixed slot.
                return Self::slot_read(previous.clone(), key, limits);
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } if key.as_constant().is_some() => {
                let yes = Self::slot_read(then_state.clone(), key.clone(), limits)?;
                let no = Self::slot_read(else_state.clone(), key, limits)?;
                return Self::ite(condition.clone(), yes, no, limits);
            }
            _ => {}
        }
        Self::retained(ExprKind::SlotRead { state, key }, nodes, depth, limits)
    }
    /// Hash a canonical content sequence. No collision-free assumption is added.
    pub fn keccak(input: ByteView, limits: ExprLimits) -> Result<Self, ExprError> {
        let input = input.canonicalize();
        let (nodes, depth) = state::metrics([(input.nodes(), input.depth())]);
        state::check(nodes, depth, limits.into())?;
        if input.length.as_constant() == Some(U256::ZERO) {
            return Ok(Self::constant(U256::from_be_bytes(keccak256([]).0)));
        }
        if matches!(input.state.kind(), ByteStateKind::Exact(_))
            && input
                .length
                .as_constant()
                .and_then(|n| usize::try_from(n).ok())
                .is_some_and(|n| {
                    n.saturating_mul(input.nodes().saturating_add(1)) <= limits.max_nodes
                })
            && let Some(materialized) = input.known_bytes(limits)?
        {
            return Ok(Self::constant(U256::from_be_bytes(
                keccak256(materialized).0,
            )));
        }
        Self::retained(ExprKind::Keccak { input }, nodes, depth, limits)
    }
    /// State read/hash axioms can refine a word even without path constraints.
    /// Consumers must not skip SMT merely because RelationState is empty.
    pub fn has_state_semantics(&self) -> bool {
        self.0.state_semantics
    }
}
