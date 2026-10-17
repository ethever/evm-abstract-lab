use super::{EmptyFiniteConstantSet, FiniteConstantSet, Kind};
use alloy_primitives::U256;
use std::{collections::BTreeSet, num::NonZeroUsize, sync::Arc};

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("test capacity is positive")
}

fn finite(values: &[u64]) -> FiniteConstantSet {
    FiniteConstantSet::try_from_values(values.iter().copied().map(U256::from).collect())
        .expect("test candidates are nonempty")
}

#[test]
fn empty_candidates_are_typed_contradictions() {
    assert_eq!(
        FiniteConstantSet::try_from_values(BTreeSet::new()),
        Err(EmptyFiniteConstantSet)
    );
    assert_eq!(
        FiniteConstantSet::collect_bounded([], capacity(1)),
        Err(EmptyFiniteConstantSet)
    );
    assert_eq!(
        EmptyFiniteConstantSet.to_string(),
        "finite constant set must be nonempty"
    );
}

#[test]
fn collection_charges_distinct_candidates_and_stops_at_overflow() {
    let result =
        FiniteConstantSet::collect_bounded([1, 1, 2, 2].into_iter().map(U256::from), capacity(2));
    assert_eq!(result, Ok(finite(&[1, 2])));

    let mut consumed = 0;
    let input = [1, 1, 2, 3, 4].into_iter().map(|value| {
        consumed += 1;
        U256::from(value)
    });
    assert_eq!(
        FiniteConstantSet::collect_bounded(input, capacity(2)),
        Ok(FiniteConstantSet::top())
    );
    assert_eq!(consumed, 4);
}

#[test]
fn capacity_is_external_and_can_be_applied_to_an_existing_set() {
    let set = finite(&[1, 2, 3]);
    assert_eq!(set.limit(capacity(3)), set);
    assert!(set.limit(capacity(2)).is_top());
    assert_eq!(set.cardinality(), Some(3));
    assert!(FiniteConstantSet::top().limit(capacity(1)).is_top());
    for bound in 1..=4 {
        assert_eq!(
            set.clone().into_limited(capacity(bound)),
            set.limit(capacity(bound))
        );
        assert_eq!(
            FiniteConstantSet::top().into_limited(capacity(bound)),
            FiniteConstantSet::top().limit(capacity(bound))
        );
    }

    assert_eq!(finite(&[1, 2]).join(&finite(&[2, 3]), capacity(3)), set);
    assert!(finite(&[1, 2]).join(&finite(&[2, 3]), capacity(2)).is_top());
    assert_eq!(
        FiniteConstantSet::constant(U256::from(1))
            .join(&FiniteConstantSet::constant(U256::from(1)), capacity(1)),
        finite(&[1])
    );
}

#[test]
fn top_allows_every_word_and_absorbs_join() {
    let top = FiniteConstantSet::default();
    assert!(top.is_top());
    assert!(top.contains(U256::ZERO));
    assert!(top.contains(U256::MAX));
    assert_eq!(top.as_values(), None);
    assert_eq!(top.singleton(), None);
    assert_eq!(top.cardinality(), None);
    assert_eq!(top.work_size(), 1);
    assert_eq!(top.join(&finite(&[1, 2]), capacity(3)), top);
    assert_eq!(finite(&[1, 2]).join(&top, capacity(3)), top);
}

#[test]
fn singleton_and_read_only_queries_preserve_candidates() {
    let singleton = FiniteConstantSet::constant(U256::MAX);
    assert_eq!(singleton.singleton(), Some(U256::MAX));
    assert_eq!(singleton.cardinality(), Some(1));
    assert_eq!(singleton.work_size(), 1);
    assert!(singleton.contains(U256::MAX));
    assert!(!singleton.contains(U256::ZERO));
    let pair = finite(&[7, 2]);
    assert_eq!(pair.singleton(), None);
    assert_eq!(pair.cardinality(), Some(2));
    assert_eq!(pair.work_size(), 2);
    assert_eq!(
        pair.as_values(),
        Some(&BTreeSet::from([U256::from(2), U256::from(7)]))
    );
}

#[test]
fn meet_and_filter_report_empty_results_without_saving_bottom() {
    let pair = finite(&[1, 2]);
    assert_eq!(pair.meet(&finite(&[2, 3])), Ok(finite(&[2])));
    assert_eq!(pair.meet(&finite(&[3])), Err(EmptyFiniteConstantSet));
    assert_eq!(pair.meet(&FiniteConstantSet::top()), Ok(pair.clone()));
    assert_eq!(FiniteConstantSet::top().meet(&pair), Ok(pair.clone()));
    assert_eq!(
        pair.filter(|value| value == U256::from(1)),
        Ok(finite(&[1]))
    );
    assert_eq!(pair.filter(|_| false), Err(EmptyFiniteConstantSet));

    let mut invoked = false;
    assert_eq!(
        FiniteConstantSet::top().filter(|_| {
            invoked = true;
            false
        }),
        Ok(FiniteConstantSet::top())
    );
    assert!(!invoked);
    assert_eq!(pair.cardinality(), Some(2));
}

#[test]
fn filter_without_removals_shares_the_original_candidates() {
    let set = finite(&[9, 2, 7, 4]);
    let original = set.as_values().unwrap().clone();
    let mut visited = Vec::new();
    let filtered = set
        .filter(|value| {
            visited.push(value);
            true
        })
        .unwrap();

    assert_eq!(visited, original.iter().copied().collect::<Vec<_>>());
    assert_eq!(set.as_values(), Some(&original));
    assert_eq!(filtered, set);
    let (Kind::Finite(original), Kind::Finite(shared)) = (&set.0, &filtered.0) else {
        panic!("unchanged candidates must remain finite");
    };
    assert!(Arc::ptr_eq(original, shared));
}

#[test]
fn filter_removals_preserve_the_prefix_and_visit_each_candidate_once_in_order() {
    let set = finite(&[5, 1, 4, 2, 3]);
    let sibling = set.clone();
    let original = set.as_values().unwrap().clone();
    let ordered: Vec<_> = original.iter().copied().collect();

    for removed_index in [0, 2, 4] {
        let mut visited = Vec::new();
        let filtered = set
            .filter(|value| {
                let index = visited.len();
                assert_eq!(value, ordered[index]);
                visited.push(value);
                index != removed_index
            })
            .unwrap();
        let expected: BTreeSet<_> = ordered
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(index, value)| (index != removed_index).then_some(value))
            .collect();

        assert_eq!(visited, ordered);
        assert_eq!(filtered.as_values(), Some(&expected));
        assert_eq!(set.as_values(), Some(&original));
        assert_eq!(sibling.as_values(), Some(&original));
        let (Kind::Finite(original), Kind::Finite(shared), Kind::Finite(changed)) =
            (&set.0, &sibling.0, &filtered.0)
        else {
            panic!("removing one of five candidates must leave finite sets");
        };
        assert!(Arc::ptr_eq(original, shared));
        assert!(!Arc::ptr_eq(original, changed));
    }
}

#[test]
fn filter_top_and_singletons_preserve_the_callback_contract() {
    let mut calls = 0;
    assert_eq!(
        FiniteConstantSet::top().filter(|_| {
            calls += 1;
            false
        }),
        Ok(FiniteConstantSet::top())
    );
    assert_eq!(calls, 0);

    let singleton = FiniteConstantSet::constant(U256::MAX);
    let mut visited = Vec::new();
    let retained = singleton
        .filter(|value| {
            visited.push(value);
            true
        })
        .unwrap();
    assert_eq!(visited, [U256::MAX]);
    assert_eq!(retained, singleton);
    let (Kind::Finite(original), Kind::Finite(shared)) = (&singleton.0, &retained.0) else {
        panic!("a retained singleton must remain finite");
    };
    assert!(Arc::ptr_eq(original, shared));

    visited.clear();
    assert_eq!(
        singleton.filter(|value| {
            visited.push(value);
            false
        }),
        Err(EmptyFiniteConstantSet)
    );
    assert_eq!(visited, [U256::MAX]);
    assert_eq!(singleton.singleton(), Some(U256::MAX));
}

#[test]
fn filter_matches_collect_for_all_small_sets_and_predicate_masks() {
    for set_mask in 1_u8..16 {
        let values: BTreeSet<_> = (0_u8..4)
            .filter(|bit| set_mask & (1 << bit) != 0)
            .map(U256::from)
            .collect();
        let set = FiniteConstantSet::try_from_values(values.clone()).unwrap();
        for predicate_mask in 0_u8..16 {
            let accepts = |value: U256| predicate_mask & (1 << value.to::<u8>()) != 0;
            let expected = FiniteConstantSet::try_from_values(
                values
                    .iter()
                    .copied()
                    .filter(|value| accepts(*value))
                    .collect(),
            );
            let mut visited = Vec::new();
            let filtered = set.filter(|value| {
                visited.push(value);
                accepts(value)
            });

            assert_eq!(
                filtered, expected,
                "set mask {set_mask:04b}, predicate mask {predicate_mask:04b}"
            );
            assert_eq!(visited, values.iter().copied().collect::<Vec<_>>());
            assert_eq!(set.as_values(), Some(&values));
            if let Ok(filtered) = filtered {
                let (Kind::Finite(original), Kind::Finite(result)) = (&set.0, &filtered.0) else {
                    panic!("a nonempty filtered set must remain finite");
                };
                assert_eq!(
                    Arc::ptr_eq(original, result),
                    set_mask & !predicate_mask == 0
                );
            }
        }
    }
}

#[test]
fn bounded_join_is_commutative_associative_idempotent_and_sound() {
    for bound in 1..=4 {
        let cap = capacity(bound);
        let mut sets = vec![FiniteConstantSet::top()];
        for mask in 1_u8..16 {
            let values = (0..4)
                .filter(|bit| mask & (1 << bit) != 0)
                .map(U256::from)
                .collect();
            let set = FiniteConstantSet::try_from_values(values).unwrap();
            if set.cardinality().unwrap() <= bound {
                sets.push(set);
            }
        }
        for left in &sets {
            assert_eq!(left.join(left, cap), *left);
            for right in &sets {
                let joined = left.join(right, cap);
                assert_eq!(joined, right.join(left, cap));
                for word in [
                    U256::ZERO,
                    U256::from(1),
                    U256::from(2),
                    U256::from(3),
                    U256::MAX,
                ] {
                    if left.contains(word) || right.contains(word) {
                        assert!(joined.contains(word));
                    }
                }
                for third in &sets {
                    assert_eq!(
                        joined.join(third, cap),
                        left.join(&right.join(third, cap), cap)
                    );
                }
            }
        }
    }
}

#[test]
fn serialized_and_displayed_candidates_keep_the_existing_contract() {
    let set = finite(&[7, 2]);
    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        serde_json::to_value(BTreeSet::from([U256::from(2), U256::from(7)])).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&FiniteConstantSet::top()).unwrap(),
        "\"Top\""
    );
    assert_eq!(set.to_string(), "{0x2, 0x7}");
    assert_eq!(FiniteConstantSet::top().to_string(), "⊤");
}

#[test]
fn byte_candidate_clones_share_the_complete_immutable_set() {
    let set =
        FiniteConstantSet::collect_bounded((0_u16..256).map(U256::from), capacity(256)).unwrap();
    let clones: Vec<_> = (0..64).map(|_| set.clone()).collect();
    let Kind::Finite(original) = &set.0 else {
        panic!("byte candidates must remain finite");
    };
    for clone in &clones {
        let Kind::Finite(shared) = &clone.0 else {
            panic!("cloning cannot lose byte candidates");
        };
        assert!(Arc::ptr_eq(original, shared));
        assert!(std::ptr::eq(
            set.as_values().unwrap(),
            clone.as_values().unwrap()
        ));
        assert_eq!(clone.cardinality(), Some(256));
        assert_eq!(clone.work_size(), 256);
    }
    let independently_collected =
        FiniteConstantSet::try_from_values(set.as_values().unwrap().clone()).unwrap();
    assert_eq!(set, independently_collected);
    let Kind::Finite(independent) = &independently_collected.0 else {
        panic!("independent byte candidates must remain finite");
    };
    assert!(!Arc::ptr_eq(original, independent));
    drop(set);
    let retained = clones.last().unwrap();
    assert_eq!(retained.cardinality(), Some(256));
    assert!(retained.contains(U256::ZERO));
    assert!(retained.contains(U256::from(255)));
}

#[test]
fn derived_byte_candidate_sets_leave_shared_siblings_unchanged() {
    let original =
        FiniteConstantSet::collect_bounded((0_u16..256).map(U256::from), capacity(256)).unwrap();
    let sibling = original.clone();
    let even = sibling
        .filter(|value| value & U256::ONE == U256::ZERO)
        .unwrap();
    let upper =
        FiniteConstantSet::collect_bounded((128_u16..256).map(U256::from), capacity(256)).unwrap();
    let intersection = sibling.meet(&upper).unwrap();
    let joined = even.join(&intersection, capacity(256));

    assert_eq!(even.cardinality(), Some(128));
    assert!(even.contains(U256::ZERO));
    assert!(!even.contains(U256::from(255)));
    assert_eq!(intersection.cardinality(), Some(128));
    assert!(!intersection.contains(U256::ZERO));
    assert!(intersection.contains(U256::from(255)));
    assert_eq!(joined.cardinality(), Some(192));
    assert_eq!(joined.work_size(), 192);
    assert!(joined.limit(capacity(191)).is_top());
    for unchanged in [&original, &sibling] {
        assert_eq!(unchanged.cardinality(), Some(256));
        assert_eq!(unchanged.work_size(), 256);
        assert!(unchanged.contains(U256::from(255)));
        assert_eq!(unchanged, &original);
    }
    assert!(std::ptr::eq(
        original.as_values().unwrap(),
        sibling.as_values().unwrap()
    ));
    assert_eq!(sibling.filter(|_| false), Err(EmptyFiniteConstantSet));
    assert_eq!(sibling.cardinality(), Some(256));
}

#[test]
fn shared_byte_candidates_keep_the_existing_json_and_debug_shape() {
    let set =
        FiniteConstantSet::collect_bounded((0_u16..256).map(U256::from), capacity(256)).unwrap();
    let expected = serde_json::Value::Array(
        (0_u16..256)
            .map(|value| serde_json::Value::String(format!("0x{value:x}")))
            .collect(),
    );
    assert_eq!(serde_json::to_value(&set).unwrap(), expected);
    assert_eq!(serde_json::to_value(set.clone()).unwrap(), expected);
    assert_eq!(
        format!("{set:?}"),
        format!("FiniteConstantSet(Finite({:?}))", set.as_values().unwrap())
    );
}
