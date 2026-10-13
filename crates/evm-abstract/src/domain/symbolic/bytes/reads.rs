//! Demand-local normalization leaves the original snapshot and its sources intact.

use super::{ByteStateId, ByteStateKind, StateError, StateLimits, state};
use alloy_primitives::U256;

impl ByteStateId {
    /// Skip only writes whose concrete mathematical ranges cannot affect this
    /// read. Unproved ranges and other state operations keep their dependency.
    pub(in crate::domain::symbolic) fn without_disjoint_writes(
        &self,
        start: U256,
        width: usize,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        let end = start.saturating_add(U256::from(width - 1));
        let mut cursor = self;
        loop {
            state::check(cursor.nodes(), cursor.depth(), limits)?;
            let Some(previous) = cursor.prior_to_disjoint_write(start, end) else {
                return Ok(cursor.clone());
            };
            cursor = previous;
        }
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
