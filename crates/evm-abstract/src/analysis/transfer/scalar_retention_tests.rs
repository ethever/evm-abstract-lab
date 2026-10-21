//! Optional scalar terms may be forgotten only after their numeric transfer.

#[cfg(test)]
use crate::analysis::testing::{Account, TestValue};
use crate::domain::ProductAlgebra;

use crate::analysis::testing::ByteArray;
use crate::analysis::testing::Entry;
use crate::analysis::testing::World;

use super::*;
use crate::{
    Fork,
    domain::{
        ReductionStatus,
        identity::Symbol,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprKind},
    },
};

fn fixture(
    code: &str,
    max_nodes: usize,
    max_depth: usize,
) -> (
    World,
    Entry,
    ExecutionConfig,
    crate::domain::ValueAlgebra<ProductAlgebra>,
) {
    let owner = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "scalar retention");
    world
        .insert(owner, Account::from_hex(code, Fork::Osaka).unwrap())
        .unwrap();
    let mut entry = Entry::new(owner);
    entry.environment.calldata = ByteArray::empty();
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_nodes = max_nodes;
    config_numeric.relations.max_depth = max_depth;
    (
        world,
        entry,
        config,
        crate::analysis::testing::product(config_numeric.spec()),
    )
}

#[test]
fn scalar_retention_limit_keeps_numeric_facts_and_completes_the_instruction() {
    // A following block lets us inspect the result before transaction finalization.
    let (world, entry, config, algebra) = fixture("015b00", 2, 64);
    let domain = algebra.borrowed();
    let mut input = TestValue::unsigned_range(U256::from(2), U256::from(3))
        .unwrap()
        .with_symbol(Symbol::CallValue, Some(7));
    input.set_origin(Origin::CallValue);
    let args = [constant(5), input.clone()];
    let reduction = domain.apply_detailed(opcode::ADD, &args);
    assert!(matches!(
        reduction.symbolic_error,
        Some(ExprError::Limit { .. })
    ));
    assert!(matches!(
        reduction.status,
        ReductionStatus::RoundLimit | ReductionStatus::FactLimit | ReductionStatus::SymbolicLimit
    ));
    let mut payload = initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![input, constant(5)];
    let mut budget = WorkBudget::new(config.max_work);
    let result = execute(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert_eq!(result.successors.len(), 1);
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    let value = &result.payload.active().stack[0];
    assert_eq!(value.numeric(), reduction.value.numeric());
    assert_eq!(value.provenance(), reduction.value.provenance());
    assert!(value.contains(U256::from(7)) && value.contains(U256::from(8)));
    assert!(!value.contains(U256::from(6)) && !value.contains(U256::from(9)));
    assert!(!value.symbolic_limit_reached());
    assert!(matches!(
        value.expression().unwrap().kind(),
        ExprKind::Fresh(_)
    ));
    assert!(result.diagnostics.contains(&(
        0,
        DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit),
    )));
    assert!(
        result
            .diagnostics
            .contains(&(0, DiagnosticKind::FactExchangeLimited(reduction.status)))
    );
    assert!(budget.used() >= domain.operation_work(opcode::ADD, &args));
    assert!(!budget.exhausted());

    for concrete in [2, 3] {
        let mut payload = initial(&world, &entry, domain).unwrap();
        payload.active_mut().stack = vec![constant(concrete), constant(5)];
        let mut budget = WorkBudget::new(config.max_work);
        let control = execute(
            &world,
            &entry,
            payload,
            &config,
            domain,
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(control.frontiers.is_empty());
        let exact = control.payload.active().stack[0].singleton().unwrap();
        assert_eq!(exact, U256::from(concrete + 5));
        assert!(value.contains(exact));
        assert!(control.diagnostics.is_empty());
    }
}

#[test]
fn forgotten_results_have_independent_leaves_while_copies_and_both_branches_survive() {
    // First block: a+1, DUP. Second: (a+1)+1, b+1, compare the saved a+1
    // with a copy of b+1, then branch. Only the two deep-input ADDs exceed depth 2.
    let (world, entry, config, algebra) =
        fixture("600101805b60010191600101808214601357005b00", 1024, 2);
    let domain = algebra.borrowed();
    let deep = || {
        TestValue::top().with_expression(
            ExprId::operation(opcode::NOT, &[ExprId::fresh().unwrap()], Default::default())
                .unwrap(),
        )
    };
    let mut payload = initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![deep(), deep()];
    let mut budget = WorkBudget::new(config.max_work);
    let prefix = execute(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert!(prefix.frontiers.is_empty(), "{:?}", prefix.frontiers);
    assert_eq!(prefix.successors.len(), 1);
    let stack = &prefix.payload.active().stack;
    assert_eq!(stack[1].expression(), stack[2].expression());
    assert!(matches!(
        stack[1].expression().unwrap().kind(),
        ExprKind::Fresh(_)
    ));
    assert!(stack[1].same_identity(&stack[2]));
    assert!(prefix.diagnostics.contains(&(
        2,
        DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit),
    )));
    let result = execute(
        &world,
        &entry,
        prefix.successors.into_iter().next().unwrap().payload,
        &config,
        domain,
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    assert!(result.diagnostics.contains(&(
        11,
        DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit),
    )));
    let stack = &result.payload.active().stack;
    let (derived, first, second) = (&stack[0], &stack[1], &stack[2]);
    assert!(matches!(
        second.expression().unwrap().kind(),
        ExprKind::Fresh(_)
    ));
    assert_ne!(first.expression(), second.expression());
    assert!(!first.same_identity(second));
    assert!(matches!(derived.expression().unwrap().kind(),
        ExprKind::Operation { opcode: opcode::ADD, args }
            if args.contains(first.expression().unwrap())));
    assert!(stack.iter().all(|value| !value.symbolic_limit_reached()));
    let comparison = result
        .instruction_values
        .iter()
        .find(|value| result.executed_pcs[value.instruction_index()] == 14)
        .unwrap();
    assert!(comparison.numeric().contains(U256::ZERO));
    assert!(comparison.numeric().contains(U256::ONE));
    for edge in [EdgeKind::BranchTrue, EdgeKind::BranchFalse] {
        assert!(
            result
                .successors
                .iter()
                .any(|successor| successor.kind == MachineEdgeKind::Intraprocedural(edge))
        );
    }
    assert_eq!(result.successors.len(), 2);

    // Equal and unequal independent concrete inputs witness the two edges.
    for (b, edge) in [(0, EdgeKind::BranchTrue), (1, EdgeKind::BranchFalse)] {
        let mut payload = initial(&world, &entry, domain).unwrap();
        payload.active_mut().stack = vec![constant(b), zero()];
        let mut budget = WorkBudget::new(config.max_work);
        let prefix = execute(
            &world,
            &entry,
            payload,
            &config,
            domain,
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(prefix.frontiers.is_empty());
        let control = execute(
            &world,
            &entry,
            prefix.successors.into_iter().next().unwrap().payload,
            &config,
            domain,
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(control.frontiers.is_empty());
        assert_eq!(control.successors.len(), 1);
        assert_eq!(
            control.successors[0].kind,
            MachineEdgeKind::Intraprocedural(edge)
        );
    }
}

#[test]
fn inherited_scalar_limit_still_records_a_frontier_and_keeps_its_marker() {
    let (world, entry, config, algebra) = fixture("015b00", 2, 64);
    let domain = algebra.borrowed();
    let input = TestValue::unsigned_range(U256::from(2), U256::from(3))
        .unwrap()
        .with_symbol(Symbol::CallValue, Some(7))
        .with_symbolic_limit();
    let mut payload = initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![input, constant(5)];
    let mut budget = WorkBudget::new(config.max_work);
    let result = execute(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert_eq!(result.successors.len(), 1);
    assert!(result.frontiers.iter().any(|frontier| {
        frontier.pc == 0
            && frontier.reason == FrontierReason::Relations(QueryReason::ExpressionLimit)
    }));
    assert!(result.payload.active().stack[0].symbolic_limit_reached());
    assert!(!result.diagnostics.iter().any(|(_, kind)| {
        *kind == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
    }));
}
