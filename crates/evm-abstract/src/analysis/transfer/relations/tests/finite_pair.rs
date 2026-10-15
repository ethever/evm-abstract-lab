//! Exercise the actual refine entry, with numeric bounds absent from its SMT state.

use super::{
    AbstractValue, Domain, ExecutionConfig, ExprId, FrontierReason, RelationState, TransferContext,
    U256, WorkBudget, opcode,
};
use crate::domain::relational::{CheckResult, ScalarQuery, ValueQuery};
use std::collections::BTreeSet;

mod boundaries;

fn pair(expression: &ExprId, left: u64, right: u64) -> AbstractValue {
    Domain::default()
        .join(
            &AbstractValue::constant(U256::from(left)),
            &AbstractValue::constant(U256::from(right)),
        )
        .with_expression(expression.clone())
}

fn operation(op: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(op, args, Default::default()).unwrap()
}

fn fixture(config: &ExecutionConfig) -> (ExprId, RelationState) {
    let expression = ExprId::fresh().unwrap();
    let branch = |constant| {
        let equality = operation(
            opcode::EQ,
            &[expression.clone(), ExprId::constant(U256::from(constant))],
        );
        let mut state = RelationState::default();
        assert_eq!(
            state.assume(&equality, true, &config.analysis.relations),
            CheckResult::Sat
        );
        state
    };
    let state = branch(2u64)
        .join_guarded(
            &branch(3),
            ExprId::fresh().unwrap(),
            &config.analysis.relations,
        )
        .unwrap();
    assert_eq!(
        state.unique_value(&expression, &config.analysis.relations),
        ValueQuery::Multiple
    );
    (expression, state)
}

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

fn prefix(
    value: &AbstractValue,
    expression: &ExprId,
    state: &RelationState,
    context: &TransferContext<'_>,
) -> usize {
    state.work_size()
        + expression.work_size()
        + context.domain.refinement_work(value, state.len())
        + context.query_cache.work(state, expression)
}

fn fee(expression: &ExprId, state: &RelationState, config: &ExecutionConfig) -> usize {
    state.work_size() + expression.work_size() + config.analysis.relations.rlimit as usize
}

#[test]
fn unasserted_finite_upper_bounds_check_both_one_or_no_candidates() {
    let config = ExecutionConfig::default();
    let (expression, state) = fixture(&config);
    // The SMT state always admits 2 and 3; none of these numeric bounds is
    // imported into it. Two models of that weaker state do not witness {0,1}.
    for (left, right, expected) in [
        (2, 3, Some(BTreeSet::from([U256::from(2), U256::from(3)]))),
        (1, 2, Some(BTreeSet::from([U256::from(2)]))),
        (0, 1, None),
    ] {
        let before = pair(&expression, left, right);
        assert_eq!(
            state.refine_numeric(
                &expression,
                before.numeric(),
                Domain::default(),
                &config.analysis.relations
            ),
            ScalarQuery::Unchanged
        );
        let mut value = before.clone();
        let mut budget = WorkBudget::new(20_000_000);
        let mut context = context(&config, &mut budget);
        let prelude = prefix(&before, &expression, &state, &context);
        assert_eq!(
            super::super::refine(&mut value, &state, &mut context),
            Ok(expected.is_some())
        );
        assert_eq!(
            context
                .query_cache
                .get(&state, &expression, &config.analysis.relations),
            expected
                .as_ref()
                .filter(|values| values.len() == 2)
                .map(|_| ValueQuery::Multiple),
            "only two distinct SAT witnesses establish a global answer"
        );
        if let Some(expected) = expected {
            assert_eq!(value.constants(), Some(&expected));
            assert_eq!(value.expression(), Some(&expression));
            assert_eq!(value.provenance(), before.provenance());
        } else {
            assert_eq!(value, before);
        }
        assert_eq!(
            budget.used() - budget.smt_cache_stats().maintenance_work as usize,
            prelude + 2 * fee(&expression, &state, &config),
            "two charged candidates, without a prepaid model/exclusion pair"
        );
        assert!(!budget.exhausted());
    }
}

#[test]
fn numeric_local_singletons_do_not_poison_later_global_unique_lookups() {
    let config = ExecutionConfig::default();
    let (expression, state) = fixture(&config);
    let mut budget = WorkBudget::new(20_000_000);
    let mut context = context(&config, &mut budget);
    for (left, right, expected) in [(1, 2, 2), (3, 4, 3)] {
        let mut value = pair(&expression, left, right);
        assert_eq!(
            super::super::refine(&mut value, &state, &mut context),
            Ok(true)
        );
        assert_eq!(value.singleton(), Some(U256::from(expected)));
        assert_eq!(
            context
                .query_cache
                .get(&state, &expression, &config.analysis.relations),
            None
        );
    }
    assert_eq!(budget.smt_cache_stats().unique_hits, 0);
    assert_eq!(budget.smt_cache_stats().candidate_hits, 0);
}

#[test]
fn local_scalar_projection_wins_before_pair_queries_or_cache_lookup() {
    let config = ExecutionConfig::default();
    let expression = ExprId::fresh().unwrap();
    let equality = operation(
        opcode::EQ,
        &[expression.clone(), ExprId::constant(U256::from(2))],
    );
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&equality, true, &config.analysis.relations),
        CheckResult::Sat
    );
    let mut value = pair(&expression, 2, 3);
    let mut budget = WorkBudget::new(fee(&expression, &state, &config) / 2);
    assert_eq!(
        super::super::refine(&mut value, &state, &mut context(&config, &mut budget)),
        Ok(true)
    );
    assert_eq!(value.singleton(), Some(U256::from(2)));
    assert!(budget.used() < fee(&expression, &state, &config));
    assert_eq!(budget.smt_cache_stats().entries, 0);
    assert!(!budget.exhausted());
}

#[test]
fn an_existing_global_boolean_proof_wins_before_new_pair_queries() {
    let config = ExecutionConfig::default();
    let (input, state) = fixture(&config);
    let expression = operation(opcode::EQ, &[input, ExprId::constant(U256::from(2))]);
    let before = pair(&expression, 0, 1);
    assert_eq!(
        state.refine_numeric(
            &expression,
            before.numeric(),
            Domain::default(),
            &config.analysis.relations
        ),
        ScalarQuery::Unchanged
    );
    let proof = state.unique_value(&expression, &config.analysis.relations);
    assert_eq!(proof, ValueQuery::Multiple);
    let mut budget = WorkBudget::new(20_000_000);
    let mut context = context(&config, &mut budget);
    context
        .query_cache
        .remember(&state, &expression, &config.analysis.relations, &proof);
    let prelude = prefix(&before, &expression, &state, &context);
    let mut value = before.clone();
    assert_eq!(
        super::super::refine(&mut value, &state, &mut context),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(
        budget.used(),
        prelude,
        "the valid local proof needs no query"
    );
    assert_eq!(budget.smt_cache_stats().entries, 0);
}

#[test]
fn two_sat_boolean_witnesses_are_remembered_for_a_query_free_warm_projection() {
    let config = ExecutionConfig::default();
    let (input, state) = fixture(&config);
    let expression = operation(opcode::EQ, &[input, ExprId::constant(U256::from(2))]);
    let before = pair(&expression, 0, 1);
    let mut budget = WorkBudget::new(20_000_000);
    let mut context = context(&config, &mut budget);
    let cold_prefix = prefix(&before, &expression, &state, &context);
    let mut value = before.clone();
    assert_eq!(
        super::super::refine(&mut value, &state, &mut context),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(
        context
            .query_cache
            .get(&state, &expression, &config.analysis.relations),
        Some(ValueQuery::Multiple)
    );
    assert_eq!(
        context.budget.used() - context.budget.smt_cache_stats().maintenance_work as usize,
        cold_prefix + 2 * fee(&expression, &state, &config)
    );
    let cold_work = context.budget.used();
    let cold_maintenance = context.budget.smt_cache_stats().maintenance_work;
    let warm_prefix = prefix(&before, &expression, &state, &context);
    assert_eq!(
        super::super::refine(&mut value, &state, &mut context),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(context.budget.used() - cold_work, warm_prefix);
    assert_eq!(
        context.budget.smt_cache_stats().maintenance_work,
        cold_maintenance
    );
}
