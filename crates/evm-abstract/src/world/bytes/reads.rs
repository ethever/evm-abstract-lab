//! Fixed demands may use complete facts from their own immutable snapshot.

use crate::domain::{AbstractValue, symbolic::ExprId};
use alloy_primitives::U256;
use std::collections::BTreeMap;

/// Every World write/copy remembers its actual destination; joins retain only
/// guarantees present on both paths. Never infer through subterms or reuse a
/// proof from another snapshot, even if its root is identical.
pub(super) fn write_lower_bound(
    destination: &ExprId,
    facts: &BTreeMap<ExprId, AbstractValue>,
) -> Option<U256> {
    facts
        .get(destination)
        .map(|value| value.interval().unsigned_bounds().0)
}
