use crate::{
    domain::{
        AbstractValue,
        identity::Symbol,
        relational::{CheckResult, QueryReason, RelationLimits, RelationState, ValueQuery},
        symbolic::{ByteStateId, ExprId, SourceId, StateBinding},
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use embedded_smt::{Cancellation, with_cancellation};
use revm_bytecode::opcode;

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

#[test]
fn unrelated_input_and_fresh_reuse_only_the_same_complete_base_decision() {
    let limits = RelationLimits::default();
    // A leaf bound directly to a constant is substituted before encoding, so
    // both satisfiable bases would become the same complete true formula.
    // This bijective expression keeps the different facts in their SMT bases.
    let word = ExprId::operation(
        opcode::XOR,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::from(3))],
        limits.expressions(),
    )
    .unwrap();
    let mut first = RelationState::default();
    first
        .observe(
            &word,
            AbstractValue::constant(U256::from(7)).numeric(),
            &limits,
        )
        .unwrap();
    let mut changed = RelationState::default();
    changed
        .observe(
            &word,
            AbstractValue::constant(U256::from(9)).numeric(),
            &limits,
        )
        .unwrap();
    let mut infeasible = first.clone();
    // Observation retains both facts without precomputing bottom.
    infeasible
        .observe(
            &word,
            AbstractValue::constant(U256::from(9)).numeric(),
            &limits,
        )
        .unwrap();
    let expressions = [
        ExprId::input(1, Symbol::CallValue),
        ExprId::fresh().unwrap(),
    ];
    let mut ledger = WorkBudget::new(100_000_000);
    for (index, (state, expected)) in [
        (&first, ValueQuery::Multiple),
        (&changed, ValueQuery::Multiple),
        (&infeasible, ValueQuery::Infeasible),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(!state.is_empty());
        assert!(!state.is_bottom());
        for repeat in 0..2 {
            for (position, expression) in expressions.iter().enumerate() {
                assert!(!state.relevant(expression));
                assert_eq!(state.unique_value(expression, &limits), expected);
                assert_eq!(query(state, expression, &limits, &mut ledger), expected);
                let hits = index * 3 + repeat * 2 + position;
                assert_eq!(ledger.smt_cache_stats().candidate_hits, hits as u64);
                assert_eq!(ledger.smt_cache_stats().entries, index + 1);
                assert_eq!(ledger.smt_cache_stats().unique_hits, 0);
            }
        }
    }
    assert_eq!(ledger.smt_cache_stats().misses, 3);
    assert_eq!(ledger.smt_cache_stats().insertions, 3);
    assert!(!ledger.exhausted());
}

#[test]
fn unrelated_values_include_the_guarded_path_in_the_complete_base_key() {
    let limits = RelationLimits::default();
    let word = ExprId::fresh().unwrap();
    let selector = ExprId::fresh().unwrap();
    let branch = |number: u64| {
        let mut state = RelationState::default();
        state
            .observe(
                &word,
                AbstractValue::constant(U256::from(number)).numeric(),
                &limits,
            )
            .unwrap();
        state
    };
    let first = branch(7);
    let second = branch(9);
    let mut feasible = first
        .join_guarded(&second, selector.clone(), &limits)
        .unwrap();
    let mut infeasible = second
        .join_guarded(&first, selector.clone(), &limits)
        .unwrap();
    for state in [&mut feasible, &mut infeasible] {
        state
            .observe(
                &selector,
                AbstractValue::constant(U256::ONE).numeric(),
                &limits,
            )
            .unwrap();
        state
            .observe(
                &word,
                AbstractValue::constant(U256::from(7)).numeric(),
                &limits,
            )
            .unwrap();
        assert!(!state.is_bottom());
    }
    assert!(feasible.constraints().eq(infeasible.constraints()));
    assert_ne!(feasible.path(), infeasible.path());
    let expression = ExprId::fresh().unwrap();
    let mut ledger = WorkBudget::new(100_000_000);
    for repeat in 0..2 {
        for (state, expected) in [
            (&feasible, ValueQuery::Multiple),
            (&infeasible, ValueQuery::Infeasible),
        ] {
            assert!(!state.relevant(&expression));
            assert_eq!(state.unique_value(&expression, &limits), expected);
            assert_eq!(query(state, &expression, &limits, &mut ledger), expected);
        }
        assert_eq!(ledger.smt_cache_stats().candidate_hits, repeat * 2);
        assert_eq!(ledger.smt_cache_stats().entries, 2);
    }
    assert_eq!(ledger.smt_cache_stats().unique_hits, 0);
}

#[test]
fn unrelated_base_native_unknown_is_not_saved_as_a_decision() {
    let limits = RelationLimits {
        rlimit: 1,
        ..RelationLimits::default()
    };
    let condition = ExprId::operation(
        opcode::EQ,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::from(7))],
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.assume_checked(&condition, true, &limits),
        Ok(CheckResult::Unknown(QueryReason::ResourceLimit))
    );
    let mut ledger = WorkBudget::new(100_000_000);
    let expressions = [
        ExprId::input(1, Symbol::CallValue),
        ExprId::fresh().unwrap(),
    ];
    for repeat in 0..2 {
        for expression in &expressions {
            assert!(!state.relevant(expression));
            assert_eq!(
                state.unique_value(expression, &limits),
                ValueQuery::Unknown(QueryReason::ResourceLimit)
            );
            assert_eq!(
                query(&state, expression, &limits, &mut ledger),
                ValueQuery::Unknown(QueryReason::ResourceLimit)
            );
        }
        assert_eq!(ledger.smt_cache_stats().undecided, (repeat + 1) * 2);
        assert_eq!(ledger.smt_cache_stats().entries, 0);
        assert_eq!(ledger.smt_cache_stats().candidate_hits, 0);
        assert_eq!(ledger.smt_cache_stats().unique_hits, 0);
        assert!(!state.is_bottom());
    }
}

#[test]
fn unrelated_warm_base_cannot_bypass_encoding_limits_or_cancellation() {
    let limits = RelationLimits::default();
    let mut state = RelationState::default();
    state
        .observe(
            &ExprId::fresh().unwrap(),
            AbstractValue::constant(U256::from(7)).numeric(),
            &limits,
        )
        .unwrap();
    let expressions = [
        ExprId::input(1, Symbol::CallValue),
        ExprId::fresh().unwrap(),
    ];
    let mut ledger = WorkBudget::new(100_000_000);
    for expression in &expressions {
        assert!(!state.relevant(expression));
        assert_eq!(
            query(&state, expression, &limits, &mut ledger),
            ValueQuery::Multiple
        );
    }
    assert_eq!(ledger.smt_cache_stats().candidate_hits, 1);
    let before = ledger.smt_cache_stats();
    let tiny = RelationLimits {
        max_nodes: 1,
        ..limits
    };
    for expression in &expressions {
        assert_eq!(
            state.unique_value(expression, &tiny),
            ValueQuery::Unknown(QueryReason::ExpressionLimit)
        );
        assert_eq!(
            query(&state, expression, &tiny, &mut ledger),
            ValueQuery::Unknown(QueryReason::ExpressionLimit)
        );
    }
    assert_eq!(ledger.smt_cache_stats(), before);
    let cancellation = Cancellation::new();
    cancellation.cancel();
    with_cancellation(&cancellation, || {
        for expression in &expressions {
            assert_eq!(
                state.unique_value(expression, &limits),
                ValueQuery::Unknown(QueryReason::Cancelled)
            );
            assert_eq!(
                query(&state, expression, &limits, &mut ledger),
                ValueQuery::Unknown(QueryReason::Cancelled)
            );
        }
    });
    assert_eq!(ledger.smt_cache_stats(), before);
}
