//! Retired metadata guards must not become artificial machine alternatives.

use super::*;
use crate::domain::NumericValue;
use std::collections::BTreeMap;

#[test]
fn twelve_same_root_guard_joins_drop_inactive_scopes_before_case_enumeration() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 2048;
    let domain = config.domain().unwrap();
    let limits = domain.spec().relations().expressions();
    let source = ExprId::fresh().unwrap();
    let mut bounded = ByteArray::memory();
    bounded
        .write_word(
            &fixtures::word(0),
            &AbstractValue::unsigned_range(U256::ONE, U256::from(9))
                .unwrap()
                .with_expression(source),
            64,
            domain,
        )
        .unwrap();
    let mut wide = bounded.clone();
    // Widen all numeric caches while keeping the exact immutable content root.
    wide.update_values(&mut |value| {
        *value = value.clone().with_numeric(NumericValue::top());
    });
    assert_eq!(bounded.state_root(), wide.state_root());
    assert!(
        wide.read_word(&fixtures::word(0), domain)
            .contains(U256::from(20))
    );
    let mut nested = bounded;
    let mut selectors = BTreeSet::new();
    let mut order = Vec::new();
    for _ in 0..12 {
        let guard = ExprId::fresh().unwrap();
        nested = nested
            .join_guarded(&wide, domain, guard.clone(), limits)
            .unwrap();
        selectors.insert(guard.clone());
        order.push(guard);
    }
    assert_eq!(nested.state_root(), wide.state_root());
    assert_eq!(nested.guard_conditions(limits).unwrap().len(), 12);

    let mut projected = nested.clone();
    projected
        .rename_fresh(
            &BTreeMap::from([(order.last().unwrap().clone(), ExprId::constant(U256::ZERO))]),
            limits,
        )
        .unwrap();
    assert!(projected.guard_conditions(limits).unwrap().is_empty());
    assert!(projected.fresh_leaves().is_disjoint(&selectors));
    assert!(
        projected
            .read_word(&fixtures::word(0), domain)
            .contains(U256::from(20))
    );

    let mut payload = fixtures::payload(domain);
    payload.active_mut().memory = projected;
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let cases =
        super::super::cases::prepare(&[], None, &payload, &selectors, domain, &mut budget).unwrap();
    assert_eq!(
        cases.len(),
        1,
        "inactive guards are not uncovered alternatives"
    );
    assert!(cases[0].fresh_leaves().is_disjoint(&selectors));

    let mut selected = nested;
    selected
        .rename_fresh(
            &order
                .into_iter()
                .map(|g| (g, ExprId::constant(U256::ONE)))
                .collect(),
            limits,
        )
        .unwrap();
    let word = selected.read_word(&fixtures::word(0), domain);
    for value in [U256::ONE, U256::from(9)] {
        assert!(word.contains(value));
    }
    assert!(!word.contains(U256::ZERO));
    assert!(!word.contains(U256::from(10)));
    assert!(selected.fresh_leaves().is_disjoint(&selectors));
    assert!(selected.guard_conditions(limits).unwrap().len() <= 1);
}
