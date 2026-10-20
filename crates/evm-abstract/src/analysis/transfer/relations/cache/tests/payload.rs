use super::*;
use crate::{
    analysis::transfer::resource_tests::words::{COPIES, Heavy, Policy, copies, reset},
    domain::{Domain, ValueAlgebra, WordElement, relational::EvmValueQuery},
};

fn query(limits: RelationLimits) -> (RelationState, ExprId, EvmValueQuery<Heavy>) {
    let state = RelationState::default();
    let expression = ExprId::operation(
        revm_bytecode::opcode::AND,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::from(255))],
        limits.expressions(),
    )
    .unwrap();
    let algebra = ValueAlgebra::new(Policy(Domain::from_spec(
        Domain::default().spec().with_relations(limits),
    )));
    let answer = state.local_value_with(&expression, &limits, algebra.borrowed());
    assert!(
        matches!(&answer, EvmValueQuery::Bounds(value) if value.owned.len() == 16_384 && value.singleton().is_none())
    );
    (state, expression, answer)
}

#[test]
fn real_large_query_answer_cannot_escape_the_memo_retention_allowance() {
    let _copies = COPIES.scope();
    let limits = RelationLimits {
        max_nodes: 32,
        ..RelationLimits::default()
    };
    let (state, expression, answer) = query(limits);
    let mut cache = QueryCache::default();
    let mut budget = WorkBudget::new(10_000_000);
    reset();
    cache.remember(&state, &expression, &limits, &answer, &mut budget);
    assert_eq!(copies().bytes, 0);
    assert!(cache.values.is_empty() && cache.snapshot.is_none());
    assert_eq!(cache.retained_work, 0);
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
    assert!(
        cache
            .get(&state, &expression, &limits, &mut budget)
            .is_none()
    );
    assert!(matches!(answer, EvmValueQuery::Bounds(_)));
}

#[test]
fn optional_retention_and_hits_pay_before_any_owned_numeric_copy() {
    let _copies = COPIES.scope();
    let limits = RelationLimits {
        max_nodes: 65_536,
        ..RelationLimits::default()
    };
    let (state, expression, answer) = query(limits);
    let mut cache = QueryCache::default();
    let mut none = WorkBudget::new(0);
    reset();
    cache.remember(&state, &expression, &limits, &answer, &mut none);
    assert_eq!(copies().bytes, 0);
    assert!(cache.values.is_empty() && cache.snapshot.is_none());
    assert!(!none.exhausted());

    let mut retention = WorkBudget::new(10_000_000);
    cache.remember(&state, &expression, &limits, &answer, &mut retention);
    assert_eq!(copies().bytes, 16_384);
    assert!(copies().quotes > 0);
    assert!(retention.used() >= copies().bytes);
    assert!(cache.retained_work >= 16_384 && cache.retained_work <= limits.max_nodes);
    let quote = cache.work(&state, &expression);
    assert!(quote >= 16_384);
    let EvmValueQuery::Bounds(value) = &answer else {
        unreachable!()
    };
    assert!(value.clone_work() > value.work_size().saturating_mul(3));
    assert!(quote >= value.clone_work());

    reset();
    let mut refused = WorkBudget::new(quote - 1);
    for _ in 0..200 {
        assert!(
            cache
                .get(&state, &expression, &limits, &mut refused)
                .is_none()
        );
    }
    assert_eq!(copies().bytes, 0);
    assert_eq!(refused.used(), 0);
    assert!(!refused.exhausted());

    let mut exact = WorkBudget::new(quote);
    assert!(
        matches!(cache.get(&state, &expression, &limits, &mut exact), Some(EvmValueQuery::Bounds(value)) if value.owned.len() == 16_384)
    );
    assert_eq!(copies().bytes, 16_384);
    assert_eq!(exact.used(), quote);
    assert!(!exact.exhausted());
    eprintln!(
        "Memo resource witness: retained_work={} retention_paid={} hit_quote={} copied_bytes={} refused_hits=200 refused_copy_bytes=0",
        cache.retained_work,
        retention.used(),
        quote,
        copies().bytes
    );
}

#[test]
fn the_old_metadata_only_six_unit_cap_cannot_retain_a_boxed_default_bound() {
    let limits = RelationLimits {
        max_nodes: 6,
        ..RelationLimits::default()
    };
    let state = RelationState::default();
    let expression = ExprId::fresh().unwrap();
    let mut cache = QueryCache::default();
    let answer = ValueQuery::Bounds(Box::new(NumericValue::top()));
    let mut budget = WorkBudget::new(10_000);
    cache.remember(&state, &expression, &limits, &answer, &mut budget);
    assert!(cache.values.is_empty() && cache.snapshot.is_none());
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
}
