use super::{
    CheckResult, Domain, ExecutionConfig, ExprId, FrontierReason, RelationState, ScalarQuery, U256,
    WorkBudget, context, fee, fixture, operation, pair, prefix,
};
use crate::domain::relational::QueryReason;
use embedded_smt::{Cancellation, with_cancellation};
use revm_bytecode::opcode;

#[test]
fn second_pair_query_work_keeps_the_input_and_pays_the_first_query() {
    let config = ExecutionConfig::default();
    let (expression, state) = fixture(&config);
    let before = pair(&expression, 1, 2);
    assert_eq!(
        state.can_equal(&expression, U256::ONE, &config.analysis.relations),
        CheckResult::Unsat
    );
    assert_eq!(
        state.can_equal(&expression, U256::from(2), &config.analysis.relations),
        CheckResult::Sat
    );
    let mut probe = WorkBudget::new(20_000_000);
    let prelude = prefix(&before, &expression, &state, &context(&config, &mut probe));
    let cost = fee(&expression, &state, &config);
    // One fee plus its optional cache maintenance cannot admit a second fee.
    let maximum = prelude + cost + cost / 4;
    let mut budget = WorkBudget::new(maximum);
    let mut value = before.clone();
    assert_eq!(
        super::super::super::refine(&mut value, &state, &mut context(&config, &mut budget)),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(
        budget.used() - budget.smt_cache_stats().maintenance_work as usize,
        prelude + cost
    );
    assert!(budget.exhausted());
    assert!(budget.used() <= maximum);
    assert!(maximum - budget.used() < cost);
}

#[test]
fn second_pair_encoding_unknown_does_not_install_the_sat_prefix() {
    let mut config = ExecutionConfig::default();
    let input = ExprId::fresh().unwrap();
    let expression = operation(
        opcode::XOR,
        &[input.clone(), ExprId::constant(U256::from(2))],
    );
    let square = operation(opcode::MUL, &[input.clone(), input]);
    let low = operation(opcode::AND, &[square, ExprId::constant(U256::from(3))]);
    let guard = operation(opcode::EQ, &[low, ExprId::constant(U256::ZERO)]);
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&guard, true, &config.analysis.relations),
        CheckResult::Sat
    );
    let boundary = (1..128)
        .find(|nodes| {
            let mut limits = config.analysis.relations;
            limits.max_nodes = *nodes;
            state.can_equal(&expression, U256::from(2), &limits) == CheckResult::Sat
                && state.can_equal(&expression, U256::from(4), &limits)
                    == CheckResult::Unknown(QueryReason::ExpressionLimit)
        })
        .expect("first constant is memoized, while the second requires one extra encoding node");
    config.analysis.relations.max_nodes = boundary;
    let before = pair(&expression, 2, 4);
    assert!(matches!(
        state.refine_numeric(
            &expression,
            before.numeric(),
            Domain::default(),
            &config.analysis.relations
        ),
        ScalarQuery::Unchanged | ScalarQuery::Unknown(QueryReason::ScalarFactLimit)
    ));
    let mut budget = WorkBudget::new(20_000_000);
    let mut context = context(&config, &mut budget);
    let prelude = prefix(&before, &expression, &state, &context);
    let mut value = before.clone();
    assert_eq!(
        super::super::super::refine(&mut value, &state, &mut context),
        Err(FrontierReason::Relations(QueryReason::ExpressionLimit))
    );
    assert_eq!(value, before);
    assert_eq!(
        budget.used() - budget.smt_cache_stats().maintenance_work as usize,
        prelude + 2 * fee(&expression, &state, &config)
    );
    assert!(!budget.exhausted());
}

#[test]
fn cancelled_root_refuses_the_first_unpaid_operation_without_narrowing() {
    let config = ExecutionConfig::default();
    let (expression, state) = fixture(&config);
    let before = pair(&expression, 1, 2);
    let token = Cancellation::new();
    token.cancel();
    let mut value = before.clone();
    with_cancellation(&token, || {
        let mut budget = WorkBudget::new(20_000_000);
        assert_eq!(
            super::super::super::refine(&mut value, &state, &mut context(&config, &mut budget)),
            Err(FrontierReason::Work)
        );
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.smt_cache_stats().entries, 0);
        assert!(budget.exhausted());
    });
    assert_eq!(value, before);
}

#[test]
fn cancellation_after_budget_capture_remains_a_typed_relational_failure() {
    let config = ExecutionConfig::default();
    let (expression, state) = fixture(&config);
    let before = pair(&expression, 1, 2);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(20_000_000);
    let prelude = prefix(&before, &expression, &state, &context(&config, &mut budget));
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            super::super::super::refine(&mut value, &state, &mut context(&config, &mut budget)),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!(value, before);
    assert_eq!(budget.used(), prelude + fee(&expression, &state, &config));
    assert_eq!(budget.smt_cache_stats().entries, 0);
    assert!(!budget.exhausted());
}
