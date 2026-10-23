//! Stage genuine saved callers at a process boundary, then use native Body/Resume.
//! These controls expose versions and checkpoints absent from public reports.

use super::*;
use crate::analysis::{BoundedJoin, KnownProcessSummaries, worklist::runtime::TaskKind};

fn child(mut payload: MachinePayload, offset: u64) -> MachinePayload {
    let mut state = payload.active().clone();
    state.key.is_static = true;
    state.key.caller = address(0x101).into();
    state.caller_operand = Some(word(0x101));
    state.stack.clear();
    state.saved_store = payload.store.snapshot();
    payload.call_stack.push_child(ChildFrame {
        state,
        continuation: Continuation {
            return_block: Some(0),
            output_offset: word(offset),
            output_size: word(32),
            creation: None,
        },
    });
    payload.normalize();
    payload
}

#[test]
fn independent_equal_programs_share_process_body_and_paid_relative_replay() {
    let state = SemanticPolicies::new(
        StructuralPartition,
        BoundedJoin,
        FeedbackAcceleration,
        KnownProcessSummaries,
    );
    let config = crate::analysis::ExecutionConfig {
        root_terminal_coverage: false,
        max_work: 20_000_000,
        ..Default::default()
    };
    let mut h = Harness::with_config("00", state, config);
    let first = h.input(0);
    let mut second = h.input(0);
    let original = first.active().program.as_ref().unwrap();
    let independent =
        crate::bytecode::Program::decode_with_fork(original.bytes(), original.fork()).unwrap();
    assert!(!original.shares_capture(&independent));
    second.active_mut().program = Some(independent.clone());
    second.store.deploy_code(address(0x101), independent);
    second.active_mut().saved_store = second.store.snapshot();
    second.normalize();
    let mut solver = h.solver();
    let caller = solver.procedure(first.clone()).unwrap();
    let left = solver
        .table
        .entry_source(caller)
        .unwrap()
        .payload
        .clone_with_payment(solver.budget)
        .unwrap();
    let mut right = left.clone_with_payment(solver.budget).unwrap();
    right.frames[0].frame.program = second.active().program.clone();
    assert!(
        left.eq_with_payment(&right, solver.domain, solver.budget)
            .unwrap()
    );
    let joined = left
        .join_bound_with_payment(&right, solver.domain, solver.budget, false)
        .unwrap();
    assert!(
        joined
            .eq_with_payment(&left, solver.domain, solver.budget)
            .unwrap()
    );
    // Public native rehydration remains forbidden; the installed adapter uses
    // its private replay only to execute a typed Process transfer.
    let replay =
        payload::caller::instantiate_with_payment(&right, solver.domain, solver.budget).unwrap();
    assert!(replay.active().key.process.is_some());
    assert_eq!(solver.procedure(second.clone()).unwrap(), caller);
    let from = solver.table.procedures[caller].first_body();
    solver.subscribe(from, child(first, 0)).unwrap();
    solver.subscribe(from, child(second, 0)).unwrap();
    assert_eq!(solver.table.subscribers.len(), 1);
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    assert!(
        solver.table.subscribers[0]
            .delivered
            .contains_key(&OutcomeKind::Return)
    );
}

#[test]
fn process_body_growth_and_template_growth_redeliver_to_two_saved_callers() {
    let state = SemanticPolicies::new(
        StructuralPartition,
        BoundedJoin,
        FeedbackAcceleration,
        KnownProcessSummaries,
    );
    let config = crate::analysis::ExecutionConfig {
        root_terminal_coverage: false,
        max_work: 20_000_000,
        ..Default::default()
    };
    // Both terminal capabilities are real decoded instructions. The process
    // admits their coarse alternatives without claiming either PC was visited.
    let mut h = Harness::with_config("60006000f360006000fd", state, config);
    let values = crate::analysis::testing::product(h.domain.spec());
    let domain = values.borrowed();
    let mut first = h.input(7);
    first
        .store
        .write(address(0x101), &word(0), &word(7), domain);
    first
        .store
        .write(address(0x101), &word(1), &word(3), domain);
    first
        .store
        .write_transient(address(0x101), &word(1), &word(0), domain);
    let mut second = first.clone();
    second
        .store
        .write(address(0x101), &word(0), &word(9), domain);
    second
        .store
        .write(address(0x101), &word(1), &word(4), domain);
    second
        .store
        .write_transient(address(0x101), &word(1), &word(4), domain);
    let mut solver = h.solver();
    let caller = solver.procedure(first.clone()).unwrap();
    let from = solver.table.procedures[caller].first_body();
    let other = solver
        .enqueue(caller, second.clone(), Some(true), from)
        .unwrap();
    assert_ne!(from, other);
    let first = child(first, 0);
    let second = child(second, 64);
    solver.subscribe(from, first.clone()).unwrap();
    let callee = solver.table.subscribers[0].callee;
    let body = solver.table.procedures[callee].first_body();
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    let body_before = solver.track_input(body).unwrap();
    let generation = solver.table.procedures[callee].outputs[&OutcomeKind::Return].generation;
    let old_delivery = solver.table.subscribers[0].delivered[&OutcomeKind::Return];
    let transfers = solver.counters.transfers;

    solver.subscribe(other, second).unwrap();
    assert_eq!(solver.table.subscribers.len(), 2);
    assert_eq!(solver.table.subscribers[1].callee, callee);
    assert!(solver.table.procedures[callee].entry_updates > 0);
    assert_ne!(solver.track_input(body).unwrap(), body_before);
    assert!(
        solver
            .runtime
            .pending_relative()
            .any(|task| task == TaskKind::Body(body))
    );
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    assert!(solver.counters.transfers > transfers);
    let grown_generation = solver.table.procedures[callee].outputs[&OutcomeKind::Return].generation;
    assert!(grown_generation > generation);
    for (subscriber, offset, slot0, slot1) in [(0, 0, 7, 3), (1, 64, 9, 4)] {
        let retained = &solver.table.subscribers[subscriber];
        assert_eq!(
            retained.payload.continuations[0].output_offset.singleton(),
            Some(U256::from(offset))
        );
        assert_eq!(
            retained.payload.continuations[0].output_size.singleton(),
            Some(U256::from(32))
        );
        let saved = retained.payload.frames[1]
            .saved_store
            .instantiate(domain)
            .unwrap();
        for (slot, expected) in [(0, slot0), (1, slot1)] {
            assert_eq!(
                saved.read(address(0x101), &word(slot), domain).singleton(),
                Some(U256::from(expected))
            );
        }
        assert_eq!(
            saved
                .read_transient(address(0x101), &word(1), domain)
                .singleton(),
            Some(U256::from(if subscriber == 0 { 0 } else { 4 }))
        );
        for (kind, output) in &solver.table.procedures[callee].outputs {
            let stamp = retained.delivered[kind];
            assert_eq!(stamp.1, output.generation);
        }
    }
    assert_eq!(
        solver.table.subscribers[0].delivered[&OutcomeKind::Return].0,
        old_delivery.0
    );

    // Only the saved caller changes. The callee input/output stay subsumed, so
    // Resume must replay the new template against the SAME output generation.
    let before = solver.table.subscribers[0].delivered[&OutcomeKind::Return];
    let body_version = solver.track_input(body).unwrap();
    let mut grown_caller = first;
    grown_caller
        .call_stack
        .iter_mut()
        .next()
        .unwrap()
        .call_value = word(3);
    solver.subscribe(from, grown_caller).unwrap();
    assert_eq!(solver.table.subscribers.len(), 2);
    assert!(
        solver
            .runtime
            .pending_relative()
            .any(|task| task == TaskKind::Resume(0))
    );
    assert_eq!(solver.track_input(body).unwrap(), body_version);
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(solver.runtime.relative_closed(solver.budget).unwrap());
    let after = solver.table.subscribers[0].delivered[&OutcomeKind::Return];
    assert_ne!(before.0, after.0);
    assert_eq!(before.1, after.1);
    let cfg = solver.regional_cfg().unwrap();
    for node in &cfg.nodes {
        let transfer = node.transfer.as_ref().unwrap();
        assert!(transfer.current && transfer.complete);
        assert_eq!(transfer.input_revision, node.input_revision);
        assert!(!transfer.visited_block);
    }
    for call in &cfg.calls {
        for delivery in &call.deliveries {
            assert!(delivery.current);
            assert_eq!(delivery.input_revision, call.input_revision);
            assert_eq!(
                delivery.output_generation,
                solver.table.procedures[call.callee].outputs[&delivery.kind].generation
            );
        }
    }
    eprintln!(
        "PROCESS_GENERATIONS work={} transfers={} body_input={} -> {} output={} -> {} template={} -> {} delivery_generation={}",
        solver.budget.used(),
        solver.counters.transfers,
        body_before.get(),
        body_version.get(),
        generation,
        grown_generation,
        before.0.get(),
        after.0.get(),
        after.1
    );
}
