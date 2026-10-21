use crate::domain::ProductAlgebra;
//! Production subscriptions depend on caller templates and callee outputs.

use super::*;
use crate::analysis::worklist::runtime::TaskKind;
use crate::domain::EvmNumericAlgebra as _;

#[test]
fn tracking_a_native_body_uses_no_carrier_copy_or_query_and_honors_cancellation() {
    use crate::resource::clone_payment::tests::{RECEIPT, Word, assert_unobserved, fixture, reset};

    let _scope = RECEIPT.scope();
    let (world, entry, config, algebra) = fixture("00");
    let mut input = super::super::super::initial(&world, &entry, algebra.borrowed()).unwrap();
    input
        .active_mut()
        .stack
        .push(EvmValue::from_numeric(Word::expensive(64)));
    input.normalize();
    let mut budget = WorkBudget::new(config.max_work);
    let mut table = Table::with_algebra(algebra);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let procedure = solver.procedure(input).unwrap();
    let body = solver.table.procedures[procedure].first_body();
    reset();
    let before = solver.budget.used();
    solver.track_input(body).unwrap();
    assert_unobserved();
    assert!(solver.budget.used() > before);
    let before = solver.budget.used();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            solver.track_input(body),
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
    });
    assert_eq!(solver.budget.used(), before);
    assert_unobserved();
}

#[test]
fn output_growth_wakes_the_retained_outer_world_consumer() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut config = crate::analysis::machine::fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.use_summaries = true;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let payload = super::super::testing::initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut table = Table::default();
    let (_, record) = table.execute(
        0,
        &world,
        &entry,
        payload.clone(),
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed);
    assert!(!table.runtime.contains_world(&0));
    let root = record.root_procedure.unwrap();
    let mut data = ByteArray::memory();
    data.write_word(&word(0), &word(37), domain).unwrap();
    table
        .solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
        .publish(root, OutcomeKind::Return, data, payload.store)
        .unwrap();
    assert!(table.runtime.contains_world(&0));
}

#[test]
fn caller_growth_redelivers_an_unchanged_callee_generation() {
    let (world, entry) = fixture(&[(0x101, "005b00")]);
    let mut config = crate::analysis::machine::fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.use_summaries = true;
    config_numeric.relations.enabled = false;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut payload = super::super::testing::initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![word(7)];
    payload.normalize();
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let caller = solver.procedure(payload.clone()).unwrap();
    let from = solver.table.procedures[caller].first_body();
    let mut child = payload.active().clone();
    child.key.basic_block_index = 1;
    child.stack.clear();
    payload.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: Some(1),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
    payload.normalize();
    solver.subscribe(from, payload.clone()).unwrap();
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    let subscriber = solver
        .table
        .subscribers_by_key
        .iter()
        .find_map(|((source, callee, _), id)| (*source == from && *callee == 1).then_some(*id))
        .unwrap();
    let callee = solver.table.subscribers[subscriber].callee;
    let generation = solver.table.procedures[callee].outputs[&OutcomeKind::Return].generation;
    let delivered = solver.table.propagations;
    let old_receipt = solver.table.subscribers[subscriber].delivered[&OutcomeKind::Return];
    payload.call_stack.iter_mut().next().unwrap().stack[0] = word(9);
    solver.subscribe(from, payload).unwrap();
    assert_eq!(
        solver.table.procedures[callee].outputs[&OutcomeKind::Return].generation,
        generation
    );
    assert!(
        solver
            .runtime
            .pending_relative()
            .any(|task| task == TaskKind::Resume(subscriber))
    );
    solver.run();
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.table.propagations > delivered);
    let receipt = solver.table.subscribers[subscriber].delivered[&OutcomeKind::Return];
    assert_ne!(receipt.0, old_receipt.0);
    assert_eq!(receipt.1, generation);
    assert!(
        solver
            .table
            .nodes
            .iter()
            .filter(|node| node.procedure == caller)
            .any(|node| node
                .payload
                .active()
                .stack
                .first()
                .is_some_and(
                    |value| value.contains(U256::from(7)) && value.contains(U256::from(9))
                ))
    );
}

#[test]
fn late_feedback_redelivers_an_unchanged_template_and_callee_output() {
    let (world, entry) = fixture(&[(0x101, "005b00")]);
    let config = crate::analysis::machine::fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut payload = super::super::testing::initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![word(7)];
    payload.normalize();
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let caller = solver.procedure(payload.clone()).unwrap();
    let from = solver.table.procedures[caller].first_body();
    let mut child = payload.active().clone();
    child.key.basic_block_index = 1;
    child.stack.clear();
    payload.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: Some(1),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
    payload.normalize();
    solver.subscribe(from, payload).unwrap();
    solver.run();
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    let subscriber = 0;
    let template = solver.table.subscribers[subscriber].payload.clone();
    let receipt = solver.table.subscribers[subscriber].delivered[&OutcomeKind::Return];
    let delivered = solver.table.propagations;

    solver.node_edge(from, from).unwrap();
    assert!(semantic_eq(
        &template,
        &solver.table.subscribers[subscriber].payload
    ));
    assert!(
        solver
            .runtime
            .pending_relative()
            .any(|task| task == TaskKind::Resume(subscriber))
    );
    solver.run();
    let next = solver.table.subscribers[subscriber].delivered[&OutcomeKind::Return];
    assert_ne!(receipt.0, next.0);
    assert_eq!(receipt.1, next.1);
    assert!(solver.table.propagations > delivered);
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
}
