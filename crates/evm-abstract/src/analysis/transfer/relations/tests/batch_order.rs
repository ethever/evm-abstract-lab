use super::{
    AbstractValue, Domain, ExecutionConfig, ExprId, FrontierReason, RelationState, TransferContext,
    U256, WorkBudget, opcode,
};
use crate::domain::relational::ScalarQuery;

fn run(
    value: &mut AbstractValue,
    relations: &RelationState,
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
    super::super::refine(value, relations, &mut context)
}

#[test]
fn actual_refine_filters_a_complete_guarded_projection_and_retains_all_on_mid_batch_work() {
    let config = ExecutionConfig::default();
    let limits = config.analysis.relations;
    let expression = ExprId::fresh().unwrap();
    let branch = |constant: u64| {
        let mut state = RelationState::default();
        let equality = ExprId::operation(
            opcode::EQ,
            &[expression.clone(), ExprId::constant(U256::from(constant))],
            Default::default(),
        )
        .unwrap();
        state.assume(&equality, true, &limits);
        state
    };
    let state = branch(2)
        .join_guarded(&branch(3), ExprId::fresh().unwrap(), &limits)
        .unwrap();
    let domain = Domain::default();
    let before = (0u64..4)
        .map(|value| AbstractValue::constant(U256::from(value)))
        .reduce(|left, right| domain.join(&left, &right))
        .unwrap()
        .with_expression(expression.clone());
    assert_eq!(
        state.refine_numeric(&expression, before.numeric(), domain, &limits),
        ScalarQuery::Unchanged
    );
    assert_eq!(
        state.unique_value(&expression, &limits),
        crate::domain::relational::ValueQuery::Multiple
    );
    let mut positive = before.clone();
    let mut complete_budget = WorkBudget::new(20_000_000);
    assert_eq!(
        run(&mut positive, &state, &config, &mut complete_budget),
        Ok(true)
    );
    assert_eq!(
        positive.constants(),
        Some(&std::collections::BTreeSet::from([
            U256::from(2),
            U256::from(3)
        ]))
    );
    assert_eq!(positive.expression(), Some(&expression));
    assert!(!complete_budget.exhausted());
    let fee = state
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(limits.rlimit as usize);
    // Strip optional maintenance from the successful control's mandatory work,
    // then allow one candidate and the bounded maintenance of unique + that
    // candidate. This cannot admit the next mandatory candidate query.
    let mandatory_prefix = complete_budget.used()
        - complete_budget.smt_cache_stats().maintenance_work as usize
        - 3 * fee;
    let maximum = mandatory_prefix + (2 * limits.rlimit as usize) / 4 + fee / 4;
    let mut limited_budget = WorkBudget::new(maximum);
    let mut limited = before.clone();
    assert_eq!(
        run(&mut limited, &state, &config, &mut limited_budget),
        Err(FrontierReason::Work)
    );
    assert_eq!(
        limited, before,
        "the UNSAT prefix must not install a partial candidate set"
    );
    assert_eq!(
        limited_budget.used() - limited_budget.smt_cache_stats().maintenance_work as usize,
        mandatory_prefix
    );
    assert!(limited_budget.used() <= maximum);
    assert!(maximum - limited_budget.used() < fee);
    assert!(limited_budget.exhausted());
}
