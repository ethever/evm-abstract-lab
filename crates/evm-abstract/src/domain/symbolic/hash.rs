//! Read-only hash content shapes, shared with diagnostics and protocol users.
//! Classification alone proves no equality: consumers need a sound local rule.

use super::{ByteStateKind, ByteView, ExprError, ExprLimits};
use serde::Serialize;
use std::collections::BTreeSet;

/// Largest concrete sequence classified as eligible for bounded byte comparison.
/// Larger symbolic/logical ranges stay lazy; this is an actual expansion bound.
pub const MAX_NORMALIZED_HASH_BYTES: usize = 64;

/// Structural content classes retained for reporting and local interpretation.
/// These tags do not certify an equality or a collision-free assumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HashSupport {
    /// Bounded exact byte-read normalization, independent of backing address.
    Normalized {
        /// Number of bytes encoded in the normalized packed content.
        bytes: usize,
    },
    /// One raw source array, its padding length, offset and requested length.
    SourceView,
    /// Historical serialized label for a fully retained byte-state DAG.
    /// No native array or provider is constructed by the core.
    NativeArray,
    /// Copy equality under range guards: a complete copied view, or an earlier
    /// view preserved by a disjoint later copy. Overlapping views stay conservative.
    GuardedCopy,
    /// Hashes of comparable alternatives, paired with the original state guard.
    GuardedState,
    /// Arbitrary lazy view content comparison is not proved by this encoding.
    /// Its hash stays conservative and stable for the identical immutable view.
    Uncompared,
}
impl ByteView {
    /// Describe retained content shape without materializing the range.
    /// Uncompared is a precision boundary, not an infeasible/unreachable path.
    pub fn hash_support(&self, limits: ExprLimits) -> Result<HashSupport, ExprError> {
        let view = self.canonicalize();
        super::state::check(view.nodes(), view.depth(), limits.into())?;
        if matches!(view.state.kind(),ByteStateKind::Copy{destination,source,length,..} if view.offset==*destination && view.length==*length && *length==source.length)
        {
            return Ok(HashSupport::GuardedCopy);
        }
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = view.state.kind()
        {
            for (state, nonzero) in [(then_state, true), (else_state, false)] {
                if view
                    .branch_view(state.clone(), condition, nonzero)
                    .0
                    .hash_support(limits)?
                    == HashSupport::Uncompared
                {
                    return Ok(HashSupport::Uncompared);
                }
            }
            return Ok(HashSupport::GuardedState);
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
        if view.prior_copy_hash_view().is_some() {
            return Ok(HashSupport::GuardedCopy);
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

    /// Select only same-guard top-level ITE operands inside a state alternative.
    /// The caller retains the outer state selector; this is not a global assumption.
    /// Returns operand traversal work to charge against the existing query budget.
    pub(crate) fn branch_view(
        &self,
        state: super::ByteStateId,
        guard: &super::ExprId,
        nonzero: bool,
    ) -> (Self, usize) {
        fn arm<'a>(
            mut expression: &'a super::ExprId,
            guard: &super::ExprId,
            nonzero: bool,
        ) -> (&'a super::ExprId, usize) {
            let mut work = 0usize;
            while let super::ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } = expression.kind()
            {
                if condition != guard {
                    break;
                }
                work = work.saturating_add(1);
                expression = if nonzero { then_value } else { else_value };
            }
            (expression, work)
        }
        let (offset, offset_work) = arm(&self.offset, guard, nonzero);
        let (length, length_work) = arm(&self.length, guard, nonzero);
        (
            Self::new(state, offset.clone(), length.clone()),
            offset_work.saturating_add(length_work),
        )
    }

    /// Earlier comparable view and the intervening copy's destination/length.
    /// Equality still needs a mathematical non-overlap guard. Arbitrary partial
    /// lazy views have no additional equality claim from this structural lookup.
    pub(crate) fn prior_copy_hash_view(&self) -> Option<(&super::ExprId, &super::ExprId, Self)> {
        let ByteStateKind::Copy {
            previous,
            destination,
            length,
            ..
        } = self.state.kind()
        else {
            return None;
        };
        let prior =
            Self::new(previous.clone(), self.offset.clone(), self.length.clone()).canonicalize();
        if matches!(prior.state.kind(), ByteStateKind::Input { .. })
            || matches!(prior.state.kind(), ByteStateKind::Copy { destination, source, length, .. }
                if prior.offset == *destination && prior.length == *length && *length == source.length)
        {
            Some((destination, length, prior))
        } else {
            None
        }
    }
}
