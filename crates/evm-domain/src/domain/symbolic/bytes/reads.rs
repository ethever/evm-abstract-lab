//! Demand-local normalization leaves the original snapshot and its sources intact.

use super::{ByteStateId, ByteStateKind, ExprId, StateError, StateLimits, state};
use alloy_primitives::U256;

impl ByteStateId {
    /// Skip only effects proved disjoint from this fixed demand. A caller may
    /// supply complete destination bounds; unproved effects keep their history.
    pub(in crate::domain::symbolic) fn without_disjoint_writes(
        &self,
        start: U256,
        width: usize,
        limits: StateLimits,
        write_lower_bound: &impl Fn(&ExprId) -> Option<U256>,
    ) -> Result<Self, StateError> {
        let end = start.saturating_add(U256::from(width - 1));
        let mut cursor = self;
        loop {
            state::check(cursor.nodes(), cursor.depth(), limits)?;
            let Some(previous) =
                cursor.prior_to_disjoint_write_with_bound(start, end, write_lower_bound)
            else {
                return Ok(cursor.clone());
            };
            cursor = previous;
        }
    }

    /// The supplied bound must cover every destination represented by this
    /// caller's immutable snapshot. It is a read proof, not a root rewrite.
    pub fn prior_to_disjoint_write_with_bound(
        &self,
        start: U256,
        end: U256,
        write_lower_bound: &impl Fn(&ExprId) -> Option<U256>,
    ) -> Option<&Self> {
        self.prior_to_disjoint_write(start, end).or_else(|| {
            let (previous, destination) = match self.kind() {
                ByteStateKind::WriteByte {
                    previous, offset, ..
                }
                | ByteStateKind::WriteWord {
                    previous, offset, ..
                }
                | ByteStateKind::Copy {
                    previous,
                    destination: offset,
                    ..
                } => (previous, offset),
                _ => return None,
            };
            write_lower_bound(destination)
                .is_some_and(|lower| end < lower)
                .then_some(previous)
        })
    }

    /// Shared concrete range proof for symbolic reads and numeric upper bounds.
    pub(crate) fn prior_to_disjoint_write(&self, start: U256, end: U256) -> Option<&Self> {
        let (previous, offset, written_width) = match self.kind() {
            ByteStateKind::WriteByte {
                previous, offset, ..
            } => (previous, offset, U256::ONE),
            ByteStateKind::WriteWord {
                previous, offset, ..
            } => (previous, offset, U256::from(32)),
            ByteStateKind::Copy {
                previous,
                destination,
                length,
                ..
            } => {
                if length.as_constant() == Some(U256::ZERO)
                    || destination.as_constant().is_some_and(|start| end < start)
                {
                    // No copied index precedes its destination, even when the
                    // length is symbolic or the source view includes padding.
                    return Some(previous);
                }
                (previous, destination, length.as_constant()?)
            }
            _ => return None,
        };
        let written_start = offset.as_constant()?;
        // Bytes past MAX are zero padding, never a wrapped low address.
        let written_end = written_start.saturating_add(written_width - U256::ONE);
        (end < written_start || written_end < start).then_some(previous)
    }
}
