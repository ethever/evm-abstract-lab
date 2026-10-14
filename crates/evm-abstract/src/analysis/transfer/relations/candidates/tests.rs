use crate::{
    analysis::{ExecutionConfig, FrontierReason, transfer::TransferContext},
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, QueryReason, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use embedded_smt::{Cancellation, with_cancellation};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

fn fixture() -> (ExprId, AbstractValue, BTreeSet<U256>) {
    let expression = ExprId::operation(
        opcode::XOR,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::from(2))],
        Default::default(),
    )
    .unwrap();
    let value = Domain::default()
        .join(
            &AbstractValue::constant(U256::from(2)),
            &AbstractValue::constant(U256::from(4)),
        )
        .with_expression(expression.clone());
    (
        expression,
        value,
        BTreeSet::from([U256::from(2), U256::from(4)]),
    )
}
fn cost(state: &RelationState, expression: &ExprId, config: &ExecutionConfig) -> usize {
    state
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(config.analysis.relations.rlimit as usize)
}
fn run(
    value: &mut AbstractValue,
    expression: &ExprId,
    candidates: &BTreeSet<U256>,
    state: &RelationState,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
) -> Result<bool, FrontierReason> {
    let mut context = TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    super::refine(value, expression, candidates, state, &mut context)
}
fn encoding_boundary(state: &RelationState, expression: &ExprId) -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    let boundary = (1..32)
        .find(|nodes| {
            let mut limits = config.analysis.relations;
            limits.max_nodes = *nodes;
            state.can_equal(expression, U256::from(2), &limits) == CheckResult::Sat
                && state.can_equal(expression, U256::from(4), &limits)
                    == CheckResult::Unknown(QueryReason::ExpressionLimit)
        })
        .expect("old independent encoding accepts memoized two but refuses fresh four");
    config.analysis.relations.max_nodes = boundary;
    config
}

#[test]
fn sat_prefix_then_work_does_not_install_a_partial_numeric_projection() {
    let state = RelationState::default();
    let (expression, before, candidates) = fixture();
    let config = ExecutionConfig::default();
    for candidate in &candidates {
        assert_eq!(
            state.can_equal(&expression, *candidate, &config.analysis.relations),
            CheckResult::Sat
        );
    }
    let fee = cost(&state, &expression, &config);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(fee);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), fee);
    assert!(budget.exhausted());
    let mut budget = WorkBudget::new(10 * fee);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(
        budget.used(),
        2 * fee + budget.smt_cache_stats().maintenance_work as usize
    );
    assert!(!budget.exhausted());
}

#[test]
fn first_native_unknown_precedes_later_encoding_or_work_failure() {
    let state = RelationState::default();
    let (expression, before, candidates) = fixture();
    let mut config = encoding_boundary(&state, &expression);
    config.analysis.relations.rlimit = 1;
    assert_eq!(
        state.can_equal(&expression, U256::from(2), &config.analysis.relations),
        CheckResult::Unknown(QueryReason::ResourceLimit)
    );
    assert_eq!(
        state.can_equal(&expression, U256::from(4), &config.analysis.relations),
        CheckResult::Unknown(QueryReason::ExpressionLimit)
    );
    let fee = cost(&state, &expression, &config);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(fee);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Err(FrontierReason::Relations(QueryReason::ResourceLimit))
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), fee);
    assert!(!budget.exhausted());
    let mut budget = WorkBudget::new(0);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), 0);
    assert!(budget.exhausted());
}

#[test]
fn encoding_failure_is_paid_individually_without_installing_the_sat_prefix() {
    let state = RelationState::default();
    let (expression, before, candidates) = fixture();
    let config = encoding_boundary(&state, &expression);
    let fee = cost(&state, &expression, &config);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(fee);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), fee);
    assert!(budget.exhausted());
    let mut budget = WorkBudget::new(10 * fee);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        ),
        Err(FrontierReason::Relations(QueryReason::ExpressionLimit))
    );
    assert_eq!(value, before);
    assert_eq!(
        budget.used(),
        2 * fee + budget.smt_cache_stats().maintenance_work as usize
    );
    assert!(!budget.exhausted());
}

#[test]
fn warm_fresh_projection_keeps_complete_numeric_results_and_pays_original_queries() {
    use crate::domain::symbolic::ExprKind;
    for rlimit in [100_000, 10_000_000] {
        let mut config = ExecutionConfig::default();
        config.analysis.relations.rlimit = rlimit;
        let mut budget = WorkBudget::new(100_000_000);
        let mut fees = 0;
        for _ in 0..2 {
            let (expression, mut value, candidates) = fixture();
            let ExprKind::Operation { args, .. } = expression.kind() else {
                unreachable!()
            };
            let mut state = RelationState::default();
            state
                .observe(
                    &args[0],
                    AbstractValue::constant(U256::ZERO).numeric(),
                    &config.analysis.relations,
                )
                .unwrap();
            assert_eq!(
                state.can_equal(&expression, U256::from(2), &config.analysis.relations),
                CheckResult::Sat
            );
            assert_eq!(
                state.can_equal(&expression, U256::from(4), &config.analysis.relations),
                CheckResult::Unsat
            );
            fees += 2 * cost(&state, &expression, &config);
            assert_eq!(
                run(
                    &mut value,
                    &expression,
                    &candidates,
                    &state,
                    &config,
                    &mut budget
                ),
                Ok(true)
            );
            assert_eq!(value.singleton(), Some(U256::from(2)));
        }
        // Both original quarter-query allowances now admit the complete key.
        assert_eq!(budget.smt_cache_stats().candidate_hits, 2);
        assert_eq!(budget.smt_cache_stats().bypasses, 0);
        assert_eq!(
            budget.used(),
            fees + budget.smt_cache_stats().maintenance_work as usize
        );
        assert!(!budget.exhausted());
    }
}

#[test]
fn captured_cancellation_refuses_work_but_paid_encoder_cancellation_stays_relational() {
    let state = RelationState::default();
    let (expression, before, candidates) = fixture();
    let config = ExecutionConfig::default();
    let fee = cost(&state, &expression, &config);
    let token = Cancellation::new();
    token.cancel();
    let mut value = before.clone();
    with_cancellation(&token, || {
        let mut budget = WorkBudget::new(2 * fee);
        assert_eq!(
            run(
                &mut value,
                &expression,
                &candidates,
                &state,
                &config,
                &mut budget
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(budget.used(), 0);
        assert!(budget.exhausted());
    });
    assert_eq!(value, before);
    // The root ledger captures its original token. Entering a new encoder token
    // later must retain the old order: charge succeeds, then encoding is Cancelled.
    let mut budget = WorkBudget::new(2 * fee);
    assert_eq!(
        with_cancellation(&token, || run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut budget
        )),
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    );
    assert_eq!(value, before);
    assert_eq!(budget.used(), fee);
    assert!(!budget.exhausted());
}
