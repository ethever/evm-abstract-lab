//! Algebra for sparse observations with explicit fallback values.
//!
//! Omitting a key means its own input's default, never absence or zero. Pointwise
//! combinations and layout projection canonicalize to the resulting default. The
//! lower-level union also supports owner-dependent fallbacks and caches whose
//! explicit entries must survive even when equal to their defaults.

use super::OrderedMap;
use std::collections::BTreeSet;

pub(super) fn union_with<K: Ord + Clone, V: Clone>(
    left: &OrderedMap<K, V>,
    right: &OrderedMap<K, V>,
    mut value: impl FnMut(&K) -> Option<V>,
) -> OrderedMap<K, V> {
    left.keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| value(&key).map(|value| (key, value)))
        .collect()
}

pub(super) fn combine<K: Ord + Clone, V: Clone + PartialEq>(
    (left, left_default): (&OrderedMap<K, V>, &V),
    (right, right_default): (&OrderedMap<K, V>, &V),
    mut combine: impl FnMut(&V, &V) -> V,
) -> (OrderedMap<K, V>, V) {
    let default = combine(left_default, right_default);
    let values = union_with(left, right, |key| {
        let value = combine(
            left.get(key).unwrap_or(left_default),
            right.get(key).unwrap_or(right_default),
        );
        (value != default).then_some(value)
    });
    (values, default)
}

pub(super) fn project<K: Ord + Clone, V: Clone + PartialEq>(
    (values, default): (&OrderedMap<K, V>, &V),
    layout: &OrderedMap<K, V>,
    mut join: impl FnMut(&V, &V) -> V,
) -> (OrderedMap<K, V>, V) {
    let mut upper = default.clone();
    for (key, value) in values {
        if layout.get(key).is_none() {
            upper = join(&upper, value);
        }
    }
    // Layout entries are coordinates only: a missing input key keeps the input
    // default, even if forgotten entries have widened the output default.
    let retained = layout
        .keys()
        .filter_map(|key| {
            let value = values.get(key).unwrap_or(default);
            (*value != upper).then_some((key.clone(), value.clone()))
        })
        .collect();
    (retained, upper)
}

// The caller supplies the default in the target representation; projecting a
// balance default must not create one shared symbol for all omitted owners.
pub(super) fn map<K: Ord + Clone, V: Clone + PartialEq>(
    (values, default): (&OrderedMap<K, V>, &V),
    mut project: impl FnMut(&V) -> V,
) -> OrderedMap<K, V> {
    values
        .iter()
        .filter_map(|(key, value)| {
            let value = project(value);
            (value != *default).then_some((key.clone(), value))
        })
        .collect()
}
