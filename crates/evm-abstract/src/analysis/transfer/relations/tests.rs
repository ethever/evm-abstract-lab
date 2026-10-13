//! Guard rejection and shared work remain coverage boundaries.

use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{Execution, TransferContext, WorkBudget, initial},
    },
    domain::{
        AbstractValue, Domain,
        relational::{QueryReason, RelationState, SmtProvider},
        symbolic::{ExprId, SlotStateId, SourceId},
    },
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

#[test]
fn boolean_state_reads_need_only_the_two_uniqueness_checks() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let mut config = ExecutionConfig::default();
        config.analysis.relations.provider = provider;
        config.analysis.relations.rlimit = 1_000_000;
        let state =
            SlotStateId::observed(SourceId::fresh().unwrap(), Vec::new(), Default::default())
                .unwrap();
        let read =
            ExprId::slot_read(state, ExprId::constant(U256::ZERO), Default::default()).unwrap();
        for code in [
            opcode::EQ,
            opcode::LT,
            opcode::GT,
            opcode::SLT,
            opcode::SGT,
            opcode::ISZERO,
        ] {
            let args = if code == opcode::ISZERO {
                vec![read.clone()]
            } else {
                vec![read.clone(), ExprId::constant(U256::ONE)]
            };
            let expression = ExprId::operation(code, &args, Default::default()).unwrap();
            let domain = Domain::default();
            let mut value = domain
                .join(
                    &AbstractValue::constant(U256::ZERO),
                    &AbstractValue::constant(U256::ONE),
                )
                .with_expression(expression);
            let before = value.clone();
            // Enough for uniqueness, deliberately insufficient for a third query.
            let mut budget = WorkBudget::new(3 * config.analysis.relations.rlimit as usize - 1);
            let mut context = TransferContext {
                config: &config,
                domain,
                budget: &mut budget,
                root_address: None,
                input_scope: None,
            };
            assert_eq!(
                super::refine(&mut value, &RelationState::default(), &mut context),
                Ok(true),
                "{provider:?}/{code}"
            );
            assert_eq!(value, before);
            assert!(!budget.exhausted());
            assert!(budget.used() >= 2 * config.analysis.relations.rlimit as usize);
        }
    }
}

#[test]
fn multiple_nonboolean_models_still_filter_all_numeric_candidates() {
    let config = ExecutionConfig::default();
    let expression = ExprId::fresh().unwrap();
    let limits = config.analysis.relations;
    let branch = |constant| {
        let mut state = RelationState::default();
        let eq = ExprId::operation(
            opcode::EQ,
            &[expression.clone(), ExprId::constant(U256::from(constant))],
            Default::default(),
        )
        .unwrap();
        state.assume(&eq, true, &limits);
        state
    };
    let relations = branch(2u64)
        .join_guarded(&branch(3), ExprId::fresh().unwrap(), &limits)
        .unwrap();
    let domain = Domain::default();
    let mut value = domain
        .join(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::ONE),
        )
        .with_expression(expression);
    let mut budget = WorkBudget::new(10_000_000);
    let mut context = TransferContext {
        config: &config,
        domain,
        budget: &mut budget,
        root_address: None,
        input_scope: None,
    };
    assert_eq!(
        super::refine(&mut value, &relations, &mut context),
        Ok(false)
    );
}
