//! Local finite exclusions install a result only after the entire scan succeeds.
use crate::{
    analysis::{ExecutionConfig, FrontierReason, transfer::TransferContext},
    domain::{
        AbstractValue, Domain,
        relational::{QueryReason, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::collections::BTreeSet;

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
fn fixture() -> (ExprId, AbstractValue, BTreeSet<U256>) {
    let expression = ExprId::fresh().unwrap();
    let value = Domain::default()
        .join(
            &AbstractValue::constant(U256::ONE),
            &AbstractValue::constant(U256::from(2)),
        )
        .with_expression(expression.clone());
    (
        expression,
        value,
        [U256::ONE, U256::from(2)].into_iter().collect(),
    )
}
#[test]
fn direct_exclusions_filter_one_or_all_without_fabricating_satisfiability() {
    let config = ExecutionConfig::default();
    let (expression, before, candidates) = fixture();
    for expected in [1u64, 3] {
        let mut state = RelationState::default();
        state.add_equal(
            &expression,
            &ExprId::constant(U256::from(expected)),
            &config.analysis.relations,
        );
        let mut value = before.clone();
        let mut budget = WorkBudget::new(10_000_000);
        assert_eq!(
            run(
                &mut value,
                &expression,
                &candidates,
                &state,
                &config,
                &mut budget
            ),
            Ok(expected == 1)
        );
        if expected == 1 {
            assert_eq!(value.singleton(), Some(U256::ONE));
            assert_eq!(value.expression(), before.expression());
        }
    }
}
#[test]
fn partial_work_failure_leaves_the_complete_old_numeric_bound() {
    let config = ExecutionConfig::default();
    let (expression, before, candidates) = fixture();
    let state = RelationState::default();
    let first = BTreeSet::from([U256::ONE]);
    let mut sample = before.clone();
    let mut first_budget = WorkBudget::new(10_000_000);
    assert_eq!(
        run(
            &mut sample,
            &expression,
            &first,
            &state,
            &config,
            &mut first_budget
        ),
        Ok(true)
    );
    let mut limited = WorkBudget::new(first_budget.used());
    let mut value = before.clone();
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &state,
            &config,
            &mut limited
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert!(limited.exhausted());
}
#[test]
fn expression_limit_and_cancellation_never_install_a_prefix() {
    let mut config = ExecutionConfig::default();
    let (expression, before, candidates) = fixture();
    config.analysis.relations.max_nodes = 1;
    let mut value = before.clone();
    let mut budget = WorkBudget::new(10_000_000);
    assert_eq!(
        run(
            &mut value,
            &expression,
            &candidates,
            &RelationState::default(),
            &config,
            &mut budget
        ),
        Err(FrontierReason::Relations(QueryReason::ExpressionLimit))
    );
    assert_eq!(value, before);
    let token = analysis_control::Cancellation::new();
    token.cancel();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(10_000_000);
        assert_eq!(
            run(
                &mut value,
                &expression,
                &candidates,
                &RelationState::default(),
                &config,
                &mut budget
            ),
            Err(FrontierReason::Work)
        );
    });
    assert_eq!(value, before);
}
