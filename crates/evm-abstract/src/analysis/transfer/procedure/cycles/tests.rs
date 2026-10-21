//! Finite acyclic joins, accelerating feedback, and the shared graph ledger.

use super::super::{
    Counters, MachineError, MachinePayload, Solver, StorageReadPolicy, Table, WorkBudget,
};
use crate::analysis::{
    ExecutionConfig,
    machine::fixtures::{fixture, word},
    transfer,
};
use alloy_primitives::U256;

fn with_solver(test: impl FnOnce(&mut Solver<'_>)) {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000,
        ..ExecutionConfig::default()
    };
    config.analysis.max_constants = 512;
    config.analysis.relations.enabled = false;
    let mut table = Table::default();
    let mut counters = Counters::default();
    let mut budget = WorkBudget::new(config.max_work);
    test(&mut table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    ));
}

fn input(solver: &Solver<'_>, block: usize, value: u64) -> MachinePayload {
    let mut payload = transfer::initial(solver.world, solver.entry, solver.domain).unwrap();
    payload.active_mut().key.basic_block_index = block;
    payload.active_mut().stack.push(word(value));
    payload
}

#[test]
fn four_acyclic_callers_keep_all_finite_return_addresses() {
    with_solver(|solver| {
        let procedure = solver.procedure(input(solver, 0, 0)).unwrap();
        let entry = solver.table.procedures[procedure].first_body();
        let mut helper = None;
        for (arm, return_pc) in [256, 272, 288, 304].into_iter().enumerate() {
            let caller = solver
                .enqueue(procedure, input(solver, arm + 1, 0), None, entry)
                .unwrap();
            helper = Some(
                solver
                    .enqueue(procedure, input(solver, 5, return_pc), None, caller)
                    .unwrap(),
            );
        }
        let helper = helper.unwrap();
        assert!(!solver.table.nodes[helper].loop_head);
        assert!(!solver.table.procedures[procedure].local_cycle);
        let targets = solver.table.nodes[helper].payload.active().stack[0]
            .constants()
            .unwrap();
        assert_eq!(
            targets.iter().copied().collect::<Vec<_>>(),
            [256, 272, 288, 304].map(U256::from)
        );
    });
}

#[test]
fn actual_local_feedback_still_accelerates_a_growing_counter() {
    with_solver(|solver| {
        let procedure = solver.procedure(input(solver, 0, 0)).unwrap();
        let entry = solver.table.procedures[procedure].first_body();
        let head = solver
            .enqueue(procedure, input(solver, 1, 0), None, entry)
            .unwrap();
        for value in 1..=4 {
            assert_eq!(
                solver
                    .enqueue(procedure, input(solver, 1, value), None, head)
                    .unwrap(),
                head
            );
        }
        assert!(solver.table.nodes[head].loop_head);
        assert!(solver.table.procedures[procedure].local_cycle);
        let counter = &solver.table.nodes[head].payload.active().stack[0];
        assert!(counter.constants().is_none());
        assert!(counter.contains(U256::from(1_000_000)));
    });
}

#[test]
fn mutual_procedure_feedback_accelerates_entry_without_marking_local_nodes() {
    with_solver(|solver| {
        let first = solver.procedure(input(solver, 0, 0)).unwrap();
        let second = solver.procedure(input(solver, 1, 0)).unwrap();
        let third = solver.procedure(input(solver, 2, 0)).unwrap();
        let entries = [first, second, third].map(|id| solver.table.procedures[id].first_body());
        solver.call_dependency(entries[0], second).unwrap();
        solver.call_dependency(entries[1], third).unwrap();
        assert!(solver.table.procedures.iter().all(|p| !p.recursive));
        solver.call_dependency(entries[2], first).unwrap();
        assert!(solver.table.procedures.iter().all(|p| p.recursive));
        assert!(solver.table.nodes.iter().all(|node| !node.loop_head));
        for value in 1..=4 {
            assert_eq!(solver.procedure(input(solver, 0, value)).unwrap(), first);
        }
        assert!(
            solver.table.nodes[entries[0]].payload.active().stack[0]
                .constants()
                .is_none()
        );
    });
}

#[test]
fn graph_discovery_charges_work_and_refuses_a_partial_backedge() {
    with_solver(|solver| {
        let procedure = solver.procedure(input(solver, 0, 0)).unwrap();
        let entry = solver.table.procedures[procedure].first_body();
        let before = solver.budget.used();
        let next = solver
            .enqueue(procedure, input(solver, 1, 0), None, entry)
            .unwrap();
        assert!(solver.budget.used() > before);
        assert!(solver.table.nodes[entry].successors.contains(&next));
        let mut exhausted = WorkBudget::new(0);
        std::mem::swap(solver.budget, &mut exhausted);
        assert_eq!(
            solver.node_edge(next, entry),
            Err(MachineError::work_failure())
        );
        assert!(solver.budget.exhausted());
        assert!(!solver.table.nodes[next].successors.contains(&entry));
        assert!(!solver.table.nodes[entry].loop_head);
    });
}

#[test]
fn unavailable_new_edge_keeps_the_real_input_frontier_without_installing_a_node() {
    with_solver(|solver| {
        let procedure = solver.procedure(input(solver, 0, 0)).unwrap();
        let entry = solver.table.procedures[procedure].first_body();
        let payload = input(solver, 1, 7);
        let summary = super::super::payload::form(&payload, solver.entry, solver.domain).unwrap();
        let mut insufficient = WorkBudget::new(summary.work_size());
        std::mem::swap(solver.budget, &mut insufficient);
        let nodes = solver.table.nodes.len();
        let error = solver
            .enqueue_summary(procedure, summary, None, Some(entry))
            .unwrap_err();
        assert_eq!(error, MachineError::work_failure());
        solver.frontier(payload.clone(), error.frontier_reason(), 0);
        assert_eq!(solver.table.nodes.len(), nodes);
        assert!(solver.table.nodes[entry].successors.is_empty());
        let frontier = solver.table.frontiers.last().unwrap();
        assert_eq!(frontier.target.as_ref(), Some(&payload.key()));
        assert_eq!(frontier.payload.as_deref(), Some(&payload));
    });
}

#[test]
fn discovering_feedback_after_execution_wakes_body_and_output_readers_once() {
    use crate::analysis::worklist::runtime::TaskKind;

    with_solver(|solver| {
        let procedure = solver.procedure(input(solver, 0, 7)).unwrap();
        let entry = solver.table.procedures[procedure].first_body();
        solver.run();
        assert!(solver.runtime.relative_closed());
        let version = solver.track_input(entry).unwrap();
        let before = solver.table.nodes[entry].payload.clone();
        let outputs = solver.table.procedures[procedure]
            .outputs
            .iter()
            .map(|(kind, output)| (*kind, (output.summary.clone(), output.generation)))
            .collect::<Vec<_>>();
        assert!(!outputs.is_empty());

        solver.node_edge(entry, entry).unwrap();
        assert!(super::super::semantic_eq(
            &before,
            &solver.table.nodes[entry].payload
        ));
        assert_ne!(version, solver.track_input(entry).unwrap());
        assert!(
            solver
                .runtime
                .pending_relative()
                .any(|task| task == TaskKind::Body(entry))
        );
        for (kind, (summary, generation)) in &outputs {
            let output = &solver.table.procedures[procedure].outputs[kind];
            assert_eq!(output.summary, *summary);
            assert!(output.generation > *generation);
        }

        solver.run();
        assert!(solver.runtime.relative_closed());
        let version = solver.track_input(entry).unwrap();
        let generations = solver.table.procedures[procedure]
            .outputs
            .values()
            .map(|output| output.generation)
            .collect::<Vec<_>>();
        solver.node_edge(entry, entry).unwrap();
        assert_eq!(version, solver.track_input(entry).unwrap());
        assert_eq!(
            generations,
            solver.table.procedures[procedure]
                .outputs
                .values()
                .map(|output| output.generation)
                .collect::<Vec<_>>()
        );
        assert!(solver.runtime.relative_closed());
    });
}
