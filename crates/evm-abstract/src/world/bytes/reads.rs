//! Fixed demands may use complete facts from their own immutable snapshot.

use crate::domain::{AbstractValue, symbolic::ExprId};
use alloy_primitives::U256;
use std::collections::BTreeMap;

/// Every World write/copy remembers its actual destination; joins merge all entries
/// for the same term. A branch-only entry is used only at a matching effect,
/// under that branch's unchanged state selector. Never infer through subterms
/// or reuse a proof from another snapshot, even if its root is identical.
pub(super) fn write_lower_bound(
    destination: &ExprId,
    facts: &BTreeMap<ExprId, AbstractValue>,
) -> Option<U256> {
    facts
        .get(destination)
        .map(|value| value.interval().unsigned_bounds().0)
}
