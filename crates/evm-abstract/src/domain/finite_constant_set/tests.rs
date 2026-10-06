use super::{EmptyFiniteConstantSet, FiniteConstantSet};
use alloy_primitives::U256;
use std::{collections::BTreeSet, num::NonZeroUsize};

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
