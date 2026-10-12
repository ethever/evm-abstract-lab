//! Read-only hash encoding support, shared with diagnostics and protocol users.

use super::{ByteStateKind, ByteView, ExprError, ExprLimits};
use serde::Serialize;
use std::collections::BTreeSet;

/// Largest concretely sized sequence normalized into a 0-based SMT byte array.
/// Larger symbolic/logical ranges stay lazy; this is an actual expansion bound.
pub const MAX_NORMALIZED_HASH_BYTES: usize = 64;

/// Content-equality evidence used by the current hash encoding. These strategies
/// add deterministic same-input relations, never collision-free assumptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HashSupport {
    /// Bounded exact byte-read normalization, independent of backing address.
    Normalized {
        /// Number of bytes expanded into the normalized content array.
        bytes: usize,
    },
    /// One raw source array, its padding length, offset and requested length.
    SourceView,
    /// Exact full-content native array plus offset and requested length.
    NativeArray,
    /// Whole-copy content equality guarded by nonwrapping destination indices.
    GuardedCopy,
    /// Arbitrary lazy view content comparison is not proved by this encoding.
    /// Its hash stays conservative and stable for the identical immutable view.
    Uncompared,
}
impl ByteView {
    /// Describe equality support without invoking SMT or materializing the range.
    /// Uncompared is a precision boundary, not an infeasible/unreachable path.
    pub fn hash_support(&self, limits: ExprLimits) -> Result<HashSupport, ExprError> {
        let view = self.canonicalize();
        super::state::check(view.nodes(), view.depth(), limits.into())?;
        if matches!(view.state.kind(),ByteStateKind::Copy{destination,source,length,..} if view.offset==*destination && view.length==*length && *length==source.length)
        {
            return Ok(HashSupport::GuardedCopy);
        }
        if let Some(bytes) = view
            .length
            .as_constant()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|n| *n <= MAX_NORMALIZED_HASH_BYTES)
        {
            return Ok(HashSupport::Normalized { bytes });
        }
        if matches!(view.state.kind(), ByteStateKind::Input { .. }) {
            return Ok(HashSupport::SourceView);
        }
        let mut stack = vec![&view.state];
        let mut seen = BTreeSet::new();
        while let Some(state) = stack.pop() {
            if !seen.insert(state.id()) {
                continue;
            }
            super::state::check(seen.len(), state.depth(), limits.into())?;
            match state.kind() {
                ByteStateKind::Zero | ByteStateKind::Exact(_) => {}
                ByteStateKind::WriteByte { previous, .. }
                | ByteStateKind::WriteWord { previous, .. } => stack.push(previous),
                ByteStateKind::Ite {
                    then_state,
                    else_state,
                    ..
                } => {
                    stack.push(then_state);
                    stack.push(else_state);
                }
                ByteStateKind::Input { .. }
                | ByteStateKind::Copy { .. }
                | ByteStateKind::Slice { .. } => return Ok(HashSupport::Uncompared),
            }
        }
        Ok(HashSupport::NativeArray)
    }
}
