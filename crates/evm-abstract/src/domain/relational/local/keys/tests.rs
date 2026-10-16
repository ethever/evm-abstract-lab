use super::super::{Engine, Work};
use super::{Bucket, KeyMap};
use crate::{
    domain::{Domain, NumericValue, relational::RelationLimits, symbolic::ExprId},
    resource::WorkBudget,
};
use alloy_primitives::U256;

#[test]
fn a_structurally_different_key_in_the_same_index_bucket_cannot_supply_a_fact() {
    let query = ExprId::fresh().unwrap();
    let foreign = ExprId::fresh().unwrap();
    assert_ne!(query, foreign);
    let limits = RelationLimits::default();
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::current_cancellation(),
        ledger: None,
    };
    let mut engine = Engine::new(&mut work, &limits, Domain::default());
    // Force an index collision independently of the real fingerprint hash.
    // A bucket match must still compare the stored expression structurally.
    engine.facts = KeyMap {
        buckets: vec![Bucket {
            fingerprint: query.cached_fingerprint(),
            entries: vec![(foreign.clone(), NumericValue::constant(U256::from(7)))],
        }],
    };
    let bounds = engine.known(&query).unwrap();
    for value in [U256::ZERO, U256::from(7), U256::from(9), U256::MAX] {
        assert!(
            bounds.contains(value),
            "collision leaked a foreign exact fact"
        );
    }
    // Importing query=9 must remain possible: a foreign query=7 cache entry
    // cannot manufacture a contradiction or overwrite its distinct subject.
    engine
        .equal(&query, &ExprId::constant(U256::from(9)))
        .unwrap();
    assert_eq!(
        engine.known(&query).unwrap().singleton(),
        Some(U256::from(9))
    );
    assert!(
        engine
            .facts
            .buckets
            .iter()
            .flat_map(|bucket| &bucket.entries)
            .any(|(key, value)| key == &foreign && value.singleton() == Some(U256::from(7)))
    );
}

#[test]
fn clear_pays_for_the_retained_entries_and_refusal_keeps_them_available() {
    let key = ExprId::fresh().unwrap();
    let mut keys = KeyMap {
        buckets: vec![Bucket {
            fingerprint: key.cached_fingerprint(),
            entries: vec![(key, 7)],
        }],
    };
    let mut ledger = WorkBudget::new(0);
    assert!(
        keys.clear(&mut Work {
            remaining: 1024,
            cancellation: analysis_control::current_cancellation(),
            ledger: Some(&mut ledger),
        })
        .is_err()
    );
    assert_eq!(keys.buckets.len(), 1);
    assert_eq!(keys.buckets[0].entries.len(), 1);
    assert_eq!(ledger.used(), 0);
    assert!(!ledger.exhausted());
    let mut ledger = WorkBudget::new(3);
    assert!(
        keys.clear(&mut Work {
            remaining: 1024,
            cancellation: analysis_control::current_cancellation(),
            ledger: Some(&mut ledger),
        })
        .is_ok()
    );
    assert!(keys.buckets.is_empty());
    assert_eq!(ledger.used(), 3);
    assert!(!ledger.exhausted());
}
