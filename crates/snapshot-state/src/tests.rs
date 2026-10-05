use crate::{Checkpoint, OrderedMap};
use proptest::prelude::{any, proptest};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::panic::{AssertUnwindSafe, catch_unwind};

type VersionPair = (Checkpoint<OrderedMap<u8, i64>>, BTreeMap<u8, i64>);

fn entries(map: &OrderedMap<u8, i64>) -> Vec<(u8, i64)> {
    map.iter().map(|(key, value)| (*key, *value)).collect()
}

proptest! {
    #[test]
    fn map_versions_match_std_after_mixed_updates(
        operations in proptest::collection::vec((0_u8..10, any::<u8>(), any::<u8>(), any::<i16>()), 0..160)
    ) {
        let mut actual = OrderedMap::new();
        let mut expected = BTreeMap::new();
        let mut versions: Vec<VersionPair> = Vec::new();
        for (operation, key, other_key, value) in operations {
            let value = i64::from(value);
            match operation {
                0 => assert_eq!(actual.insert(key, value), expected.insert(key, value)),
                1 => assert_eq!(actual.remove(&key), expected.remove(&key)),
                2 => {
                    let range = key.min(other_key)..=key.max(other_key);
                    actual.update_range(range.clone(), |_, stored| *stored += value);
                    for stored in expected.range_mut(range).map(|(_, stored)| stored) {
                        *stored += value;
                    }
                }
                3 => {
                    actual.retain(|stored_key, stored| {
                        *stored += value;
                        stored_key % 2 == key % 2
                    });
                    expected.retain(|stored_key, stored| {
                        *stored += value;
                        stored_key % 2 == key % 2
                    });
                }
                4 => versions.push((Checkpoint::capture(&actual), expected.clone())),
                5 => {
                    if !versions.is_empty() {
                        let (checkpoint, reference) = &versions[usize::from(key) % versions.len()];
                        checkpoint.clone().restore(&mut actual);
                        expected = reference.clone();
                    }
                }
                6 => {
                    if let Some(stored) = actual.get_mut(&key) {
                        *stored = value;
                    }
                    if let Some(stored) = expected.get_mut(&key) {
                        *stored = value;
                    }
                }
                7 => {
                    actual.clear();
                    expected.clear();
                }
                8 => {
                    actual.update_values(|stored| *stored += value);
                    for stored in expected.values_mut() {
                        *stored += value;
                    }
                }
                9 => {
                    let additions = [(key, value), (other_key, -value)];
                    actual.extend(additions);
                    expected.extend(additions);
                }
                _ => unreachable!(),
            }
            assert_eq!(entries(&actual), expected.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>());
            assert_eq!(actual.len(), expected.len());
            assert_eq!(actual.is_empty(), expected.is_empty());
            assert_eq!(actual.get(&key), expected.get(&key));
            assert_eq!(actual.keys().copied().collect::<Vec<_>>(), expected.keys().copied().collect::<Vec<_>>());
            assert_eq!(actual.values().copied().collect::<Vec<_>>(), expected.values().copied().collect::<Vec<_>>());
            let range = key.min(other_key)..=key.max(other_key);
            assert_eq!(actual.range(range.clone()).map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
                expected.range(range).map(|(k, v)| (*k, *v)).collect::<Vec<_>>());
            assert_eq!(serde_json::to_string(&actual).unwrap(), serde_json::to_string(&expected).unwrap());
            assert_eq!(actual.clone().into_iter().collect::<Vec<_>>(), entries(&actual));
            for (version, reference) in &versions {
                assert_eq!(entries(version.state()), reference.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>());
            }
        }
    }
}

#[test]
fn successful_grandchild_is_covered_by_outer_rollback() {
    let mut state = OrderedMap::from_iter([("caller", 3), ("child", 4), ("grandchild", 5)]);
    let before_child = Checkpoint::capture(&state);
    state.insert("child", 7);
    let before_grandchild = Checkpoint::capture(&state);
    state.insert("grandchild", 9);
    drop(before_grandchild);
    assert_eq!(state[&"grandchild"], 9);
    before_child.restore(&mut state);
    assert_eq!(state[&"caller"], 3);
    assert_eq!(state[&"child"], 4);
    assert_eq!(state[&"grandchild"], 5);
}

#[test]
fn independent_branches_restore_in_any_order() {
    let mut state = OrderedMap::from_iter([(1, vec![10]), (2, vec![20])]);
    let root = Checkpoint::capture(&state);
    state.get_mut(&1).unwrap().push(11);
    let left = Checkpoint::capture(&state);
    root.clone().restore(&mut state);
    state.get_mut(&2).unwrap().push(21);
    let right = Checkpoint::capture(&state);
    left.clone().restore(&mut state);
    assert_eq!(state[&1], vec![10, 11]);
    assert_eq!(state[&2], vec![20]);
    right.restore(&mut state);
    assert_eq!(state[&1], vec![10]);
    assert_eq!(state[&2], vec![20, 21]);
    root.restore(&mut state);
    assert_eq!(state[&1], vec![10]);
    assert_eq!(state[&2], vec![20]);
    assert_eq!(left.state()[&1], vec![10, 11]);
}

#[test]
fn duplicate_keys_ranges_and_serialization_preserve_order() {
    let mut map = OrderedMap::from_iter([(5, 50), (1, 10), (3, 30), (1, 11)]);
    assert_eq!(
        map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
        vec![(1, 11), (3, 30), (5, 50)]
    );
    assert_eq!(
        map.range(1..5).map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
        vec![(1, 11), (3, 30)]
    );
    map.update_range(1..5, |key, value| *value += key);
    assert_eq!(
        serde_json::to_string(&map).unwrap(),
        r#"{"1":12,"3":33,"5":50}"#
    );
    let saved = Checkpoint::from_state(map.clone());
    assert_eq!(
        serde_json::to_string(&saved).unwrap(),
        serde_json::to_string(&map).unwrap()
    );
    assert_eq!(
        (&map)
            .into_iter()
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<_>>(),
        map.into_iter().collect::<Vec<_>>()
    );
}

#[test]
fn mutating_callbacks_visit_entries_in_ascending_order() {
    let mut map = OrderedMap::from_iter([(5, 50), (1, 10), (3, 30)]);
    let mut visited = Vec::new();
    map.retain(|key, value| {
        visited.push(*key);
        *value += 1;
        *key != 3
    });
    assert_eq!(visited, vec![1, 3, 5]);
    visited.clear();
    map.update_range(.., |key, _| visited.push(*key));
    assert_eq!(visited, vec![1, 5]);
    let mut visited_values = Vec::new();
    map.update_values(|value| visited_values.push(*value));
    assert_eq!(visited_values, vec![11, 51]);
}

#[test]
fn invalid_ranges_panic_consistently_before_any_mutation() {
    for initial in [Vec::new(), vec![(1_u8, 10_i64), (3, 30)]] {
        let mut map = OrderedMap::from_iter(initial);
        let before = map.clone();
        for invalid in [
            (Bound::Included(3), Bound::Included(1)),
            (Bound::Excluded(1), Bound::Excluded(1)),
        ] {
            assert!(catch_unwind(|| map.range(invalid).count()).is_err());
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    map.update_range(invalid, |_, value| *value += 1);
                }))
                .is_err()
            );
            assert_eq!(map, before);
        }
    }
}

#[test]
fn empty_valid_ranges_exclude_equal_endpoints() {
    let map = OrderedMap::from_iter([(1_u8, 10_i64), (3, 30)]);
    assert_eq!(
        map.range((Bound::Included(1), Bound::Excluded(1))).count(),
        0
    );
    assert_eq!(
        map.range((Bound::Excluded(1), Bound::Included(1))).count(),
        0
    );
    assert_eq!(
        map.range((Bound::Included(1), Bound::Included(1))).count(),
        1
    );
}
