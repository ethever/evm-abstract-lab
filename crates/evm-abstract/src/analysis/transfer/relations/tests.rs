//! Guard rejection and shared work remain coverage boundaries.

#[cfg(test)]
use crate::analysis::testing::{Account, Entry, TestValue, World};

use crate::domain::ValueAlgebra;

mod noop_bounds;

use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{Execution, WorkBudget, initial},
    },
    domain::{
        ProductAlgebra,
        relational::{QueryReason, RelationState},
        symbolic::{ExprId, SlotStateId, SourceId},
    },
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn execution() -> Execution {
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "branch-limit", "unknown condition");
    world.insert(address, Account::empty()).unwrap();
    Execution {
        process: None,
        process_calls: Vec::new(),
        payload: initial(
            &world,
            &Entry::new(address),
            ValueAlgebra::new(&ProductAlgebra::default()),
        )
        .unwrap(),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: Vec::new(),
    }
}

#[test]
fn rejected_guard_keeps_the_original_coverage_frontier() {
    let mut result = execution();
    let before = result.payload.relations.clone();
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_nodes = 64;
    let mut input = ExprId::fresh().unwrap();
    for _ in 0..6 {
        input =
            ExprId::operation(opcode::ADD, &[input.clone(), input], Default::default()).unwrap();
    }
    let expression = ExprId::operation(opcode::ISZERO, &[input], Default::default()).unwrap();
    assert!(expression.nodes() > config_numeric.relations.max_nodes);
    let condition = TestValue::unsigned_range(U256::ZERO, U256::ONE)
        .unwrap()
        .with_expression(expression);
    let mut budget = WorkBudget::new(10_000_000);
    let transfer_numeric = ProductAlgebra::from_spec(config_numeric.spec());
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&transfer_numeric),
    );
    let successor = super::branch(&mut result, &condition, true, &mut context, 7).unwrap();
    assert_eq!(successor.relations, before);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(
        result.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
    assert!(result.diagnostics.is_empty());
}

#[test]
fn shared_work_exhaustion_stops_before_guard_retention() {
    let mut result = execution();
    let before = result.payload.clone();
    let config = ExecutionConfig::default();
    let condition = TestValue::top().with_expression(ExprId::fresh().unwrap());
    let mut budget = WorkBudget::new(0);
    let transfer_numeric = ProductAlgebra::default();
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&transfer_numeric),
    );
    assert!(super::branch(&mut result, &condition, true, &mut context, 7).is_none());
    assert_eq!(result.payload, before);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn unknown_boolean_state_reads_keep_both_values_without_precision_diagnostics() {
    let config = ExecutionConfig::default();
    let state =
        SlotStateId::observed(SourceId::fresh().unwrap(), Vec::new(), Default::default()).unwrap();
    let read = ExprId::slot_read(state, ExprId::constant(U256::ZERO), Default::default()).unwrap();
    let expression = ExprId::operation(opcode::ISZERO, &[read], Default::default()).unwrap();
    let mut value = TestValue::unsigned_range(U256::ZERO, U256::ONE)
        .unwrap()
        .with_expression(expression);
    let before = value.clone();
    let mut budget = WorkBudget::new(10_000_000);
    let transfer_algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut context = context(&config, &mut budget, transfer_algebra.borrowed());
    assert_eq!(
        super::refine(&mut value, &RelationState::default(), &mut context),
        Ok(true)
    );
    assert!(value.contains(U256::ZERO) && value.contains(U256::ONE));
    assert_eq!(value.expression(), before.expression());
    assert!(!budget.exhausted());
    assert!(budget.used() > 0);
}

use crate::analysis::transfer::testing::context;

#[test]
fn branch_keeps_expressions_until_instruction_consumption() {
    let config = ExecutionConfig::default();
    let expression = ExprId::fresh().unwrap();
    let condition = TestValue::top().with_expression(expression.clone());
    let mut result = execution();
    result.payload.active_mut().stack.push(condition.clone());
    let mut budget = WorkBudget::new(10_000_000);
    let transfer_algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut context = context(&config, &mut budget, transfer_algebra.borrowed());
    let mut successor = super::branch(&mut result, &condition, false, &mut context, 7).unwrap();
    assert!(result.frontiers.is_empty());
    assert!(result.diagnostics.is_empty());
    assert!(
        successor.active().stack[0].singleton().is_none(),
        "branch must not eagerly scan/project stack"
    );
    let relations = successor.relations.clone();
    assert_eq!(
        super::refine(
            &mut successor.active_mut().stack[0],
            &relations,
            &mut context
        ),
        Ok(true)
    );
    assert_eq!(successor.active().stack[0].singleton(), Some(U256::ZERO));
    assert_eq!(successor.active().stack[0].expression(), Some(&expression));
}

#[test]
fn cached_local_bounds_cannot_override_narrower_callers_or_cancellation() {
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let transfer_algebra = crate::analysis::testing::product(config_numeric.spec());
    let domain = transfer_algebra.borrowed();
    let expression = ExprId::fresh().unwrap();
    let other = ExprId::fresh().unwrap();
    let mut relations = RelationState::default();
    relations.add_equal(&expression, &other, &config_numeric.relations, domain);
    let mut value = TestValue::top().with_expression(expression.clone());
    let mut budget = WorkBudget::new(20_000_000);
    let mut context = context(&config, &mut budget, domain);
    assert_eq!(
        super::refine(&mut value, &relations, &mut context),
        Ok(true)
    );
    let mut narrowed = TestValue::unsigned_range(U256::from(7), U256::from(9))
        .unwrap()
        .with_expression(expression);
    assert_eq!(
        super::refine(&mut narrowed, &relations, &mut context),
        Ok(true)
    );
    assert!(narrowed.contains(U256::from(7)) && narrowed.contains(U256::from(9)));
    assert!(!narrowed.contains(U256::from(6)));
    let before = narrowed.clone();
    let token = analysis_control::Cancellation::new();
    token.cancel();
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            super::refine(&mut narrowed, &relations, &mut context),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!(narrowed, before);
}

#[test]
fn retained_guarded_facts_never_prune_an_admitted_numeric_candidate() {
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let expression = ExprId::fresh().unwrap();
    let limits = config_numeric.relations;
    let algebra = crate::analysis::testing::product(config_numeric.spec());
    let domain = algebra.borrowed();
    let branch = |constant| {
        let mut state = RelationState::default();
        let eq = ExprId::operation(
            opcode::EQ,
            &[expression.clone(), ExprId::constant(U256::from(constant))],
            Default::default(),
        )
        .unwrap();
        state.assume(&eq, true, &limits, domain);
        state
    };
    let relations = branch(2u64)
        .join_guarded(&branch(3), ExprId::fresh().unwrap(), &limits)
        .unwrap();
    let mut value = domain
        .join(
            &TestValue::constant(U256::ZERO),
            &TestValue::constant(U256::ONE),
        )
        .with_expression(expression);
    let mut budget = WorkBudget::new(10_000_000);
    let mut context = crate::analysis::transfer::testing::context(&config, &mut budget, domain);
    assert_eq!(
        super::refine(&mut value, &relations, &mut context),
        Ok(true)
    );
    assert!(value.contains(U256::ZERO) && value.contains(U256::ONE));
}

#[test]
fn unused_local_capacity_does_not_become_an_automatic_work_fee() {
    let expression = ExprId::fresh().unwrap();
    let other = ExprId::fresh().unwrap();
    let mut work = Vec::new();
    for nodes in [1024, 4096] {
        let config = ExecutionConfig::default();
        let mut config_numeric = crate::analysis::testing::NumericLimits::default();
        config_numeric.relations.max_nodes = nodes;
        let algebra = crate::analysis::testing::product(config_numeric.spec());
        let domain = algebra.borrowed();
        let mut relations = RelationState::default();
        relations.add_equal(&expression, &other, &config_numeric.relations, domain);
        let mut value = domain
            .join(
                &TestValue::constant(U256::ONE),
                &TestValue::constant(U256::from(2)),
            )
            .with_expression(expression.clone());
        let mut budget = WorkBudget::new(10_000_000);
        assert_eq!(
            super::refine(
                &mut value,
                &relations,
                &mut context(&config, &mut budget, domain)
            ),
            Ok(true)
        );
        assert!(value.contains(U256::ONE) && value.contains(U256::from(2)));
        assert_eq!(value.expression(), Some(&expression));
        work.push(budget.used());
    }
    assert!(work[0] > 0);
    assert_eq!(
        work[0], work[1],
        "capacity that was never traversed is not local work"
    );
}

#[test]
fn a_work_stop_mid_projection_keeps_every_original_finite_candidate() {
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let algebra = crate::analysis::testing::product(config_numeric.spec());
    let domain = algebra.borrowed();
    let expression = ExprId::fresh().unwrap();
    let other = ExprId::fresh().unwrap();
    let mut relations = RelationState::default();
    relations.add_equal(&expression, &other, &config_numeric.relations, domain);
    let before = domain
        .join(
            &TestValue::constant(U256::ONE),
            &TestValue::constant(U256::from(2)),
        )
        .with_expression(expression);
    let mut full = before.clone();
    let mut budget = WorkBudget::new(10_000_000);
    assert_eq!(
        super::refine(
            &mut full,
            &relations,
            &mut context(&config, &mut budget, domain)
        ),
        Ok(true)
    );
    let mut limited = WorkBudget::new(budget.used() - 1);
    let mut value = before.clone();
    assert_eq!(
        super::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut limited, domain)
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert!(limited.exhausted());
    assert!(limited.used() > 0);
}
