use crate::{
    domain::{
        AbstractValue,
        relational::{QueryReason, RelationLimits, RelationState, ValueQuery},
        symbolic::{ByteStateId, ExprId, SourceId, StateBinding},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;

fn observed_byte(number: u64, limits: &RelationLimits) -> ExprId {
    let source = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        ExprId::constant(U256::ONE),
        vec![StateBinding {
            index: U256::ZERO,
            value: ExprId::fresh().unwrap(),
            numeric: AbstractValue::constant(U256::from(number))
                .numeric()
                .clone(),
        }],
        limits.expressions().into(),
    )
    .unwrap();
    ExprId::byte_read(source, ExprId::constant(U256::ZERO), limits.expressions()).unwrap()
}
fn query(
    state: &RelationState,
    expression: &ExprId,
    limits: &RelationLimits,
    ledger: &mut WorkBudget,
) -> ValueQuery {
    state.unique_value_cached(
        expression,
        limits,
        &mut ledger.smt_cache_query(limits.cache_scope()),
    )
}

#[test]
fn fresh_observed_array_identities_reuse_only_the_same_complete_snapshot() {
    let limits = RelationLimits::default();
    let state = RelationState::default();
    let mut ledger = WorkBudget::new(100_000_000);
    for number in [7, 7, 9, 9] {
        let expression = observed_byte(number, &limits);
        assert_eq!(
            state.unique_value(&expression, &limits),
            ValueQuery::Unique(U256::from(number))
        );
        assert_eq!(
            query(&state, &expression, &limits, &mut ledger),
            ValueQuery::Unique(U256::from(number))
        );
    }
    assert_eq!(ledger.smt_cache_stats().unique_hits, 2);
    assert_eq!(ledger.smt_cache_stats().entries, 2);
    assert_eq!(
        ledger.used(),
        ledger.smt_cache_stats().maintenance_work as usize
    );
    assert!(!ledger.exhausted());
}

#[test]
fn a_warm_proof_does_not_bypass_the_original_encoding_limit() {
    let limits = RelationLimits::default();
    let expression = observed_byte(7, &limits);
    let state = RelationState::default();
    let mut ledger = WorkBudget::new(100_000_000);
    assert_eq!(
        query(&state, &expression, &limits, &mut ledger),
        ValueQuery::Unique(U256::from(7))
    );
    let before = ledger.smt_cache_stats();
    let tiny = RelationLimits {
        max_nodes: 1,
        ..limits
    };
    assert_eq!(
        state.unique_value(&expression, &tiny),
        ValueQuery::Unknown(QueryReason::ExpressionLimit)
    );
    assert_eq!(
        query(&state, &expression, &tiny, &mut ledger),
        ValueQuery::Unknown(QueryReason::ExpressionLimit)
    );
    assert_eq!(ledger.smt_cache_stats(), before);
}
