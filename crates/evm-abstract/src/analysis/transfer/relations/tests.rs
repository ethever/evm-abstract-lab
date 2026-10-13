//! Guard rejection and shared work remain coverage boundaries.

use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{Execution, TransferContext, WorkBudget, initial},
    },
    domain::{AbstractValue, Domain, relational::QueryReason, symbolic::ExprId},
    world::{Account, Entry, World},
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn execution() -> Execution {
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "branch-limit", "unknown condition");
    world.insert(address, Account::empty()).unwrap();
    Execution {
        payload: initial(&world, &Entry::new(address), Domain::default()).unwrap(),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
    }
}

#[test]
fn rejected_guard_keeps_the_original_coverage_frontier() {
    let mut result = execution();
    let before = result.payload.relations.clone();
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 64;
    let mut input = ExprId::fresh().unwrap();
    for _ in 0..6 {
        input =
            ExprId::operation(opcode::ADD, &[input.clone(), input], Default::default()).unwrap();
    }
    let expression = ExprId::operation(opcode::ISZERO, &[input], Default::default()).unwrap();
    assert!(expression.nodes() > config.analysis.relations.max_nodes);
    let condition = AbstractValue::unsigned_range(U256::ZERO, U256::ONE)
        .unwrap()
        .with_expression(expression);
    let mut budget = WorkBudget::new(10_000_000);
    let mut context = TransferContext {
        config: &config,
        domain: Domain::default(),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
    };
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
    let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let mut budget = WorkBudget::new(0);
    let mut context = TransferContext {
        config: &config,
        domain: Domain::default(),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
    };
    assert!(super::branch(&mut result, &condition, true, &mut context, 7).is_none());
    assert_eq!(result.payload, before);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}
