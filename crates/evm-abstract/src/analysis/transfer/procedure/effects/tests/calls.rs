//! Real CALL entry, relative saturation and per-invocation store application.

use crate::{
    analysis::{
        ExecutionConfig, MachineEdgeKind, OutcomeKind,
        engine::Counters,
        machine::fixtures::{address, fixture, word},
        transfer::{
            StorageReadPolicy,
            procedure::testing::{Table, execute_inner, initial},
        },
    },
    domain::{ProductAlgebra, ValueAlgebra},
};

#[test]
fn successful_read_only_calls_keep_their_actual_post_entry_store() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    for op in [0xf1u8, 0xf2, 0xf4, 0xfa] {
        let args = if matches!(op, 0xf1 | 0xf2) {
            "5f5f5f5f6009"
        } else {
            "5f5f5f5f"
        };
        let caller = format!("{args}6101025a{op:02x}");
        let (world, mut entry) = fixture(&[(0x101, &caller), (0x102, "00")]);
        entry.environment.value = word(13);
        let config = ExecutionConfig {
            max_work: 20_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        };
        let mut budget = crate::resource::WorkBudget::new(config.max_work);
        let mut table = Table::default();
        let mut counters = Counters::default();
        for current in [0, 1] {
            let mut input = initial(&world, &entry, domain).unwrap();
            input
                .store
                .write_checked(address(0x101), &word(0), &word(current), domain)
                .unwrap();
            input
                .store
                .write_checked(address(0x101), &word(1), &word(77), domain)
                .unwrap();
            input
                .store
                .write_checked(address(0x102), &word(0), &word(99), domain)
                .unwrap();
            input
                .store
                .write_transient_checked(address(0x101), &word(0), &word(88), domain)
                .unwrap();
            let entered = execute_inner(
                &world,
                &entry,
                input,
                &config,
                domain,
                &mut budget,
                StorageReadPolicy::Abstract,
                false,
            );
            assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
            let child = entered
                .successors
                .into_iter()
                .find(|next| next.kind == MachineEdgeKind::Call)
                .unwrap()
                .payload;
            let owner = child.active().key.address;
            assert_eq!(
                owner,
                address(if matches!(op, 0xf2 | 0xf4) {
                    0x101
                } else {
                    0x102
                })
            );
            assert_eq!(child.active().key.is_static, op == 0xfa);
            assert_eq!(
                child.active().call_value.singleton(),
                Some(crate::U256::from(if op == 0xf4 {
                    13
                } else if op == 0xfa {
                    0
                } else {
                    9
                }))
            );
            let expected = child.store.clone();
            let (execution, record) = table.execute(
                0,
                &world,
                &entry,
                child,
                &config,
                domain,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            );
            assert!(record.closed, "{:?}", execution.frontiers);
            let returned = execution
                .successors
                .iter()
                .find(|next| next.kind == MachineEdgeKind::Summary(OutcomeKind::Return))
                .unwrap();
            assert_eq!(
                returned.payload.store, expected,
                "op={op:02x}, current={current}: the joined output must not replace this invocation's roots/effects"
            );
            if op == 0xf1 {
                assert_eq!(
                    expected.read_balance(address(0x101)).singleton(),
                    Some(crate::U256::from(999_991))
                );
                assert_eq!(
                    expected.read_balance(address(0x102)).singleton(),
                    Some(crate::U256::from(1_000_009))
                );
            }
        }
    }
}

#[test]
fn pure_revert_and_exception_restore_each_actual_entry_checkpoint() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    for (callee, kind) in [
        ("5f5ffd", OutcomeKind::Revert),
        ("5f56", OutcomeKind::Failure),
    ] {
        // The second program fails its JUMP to a non-JUMPDEST; both programs
        // still satisfy the complete no-state-effects instruction proof.
        for op in [0xf1u8, 0xf2, 0xf4, 0xfa] {
            let args = if matches!(op, 0xf1 | 0xf2) {
                "5f5f5f5f6009"
            } else {
                "5f5f5f5f"
            };
            let caller = format!("{args}6101025a{op:02x}");
            let (world, entry) = fixture(&[(0x101, &caller), (0x102, callee)]);
            let config = ExecutionConfig {
                max_work: 20_000_000,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            };
            let mut budget = crate::resource::WorkBudget::new(config.max_work);
            let mut table = Table::default();
            let mut counters = Counters::default();
            for current in [0, 1] {
                let mut input = initial(&world, &entry, domain).unwrap();
                input
                    .store
                    .write_checked(address(0x101), &word(0), &word(current), domain)
                    .unwrap();
                input
                    .store
                    .write_checked(address(0x101), &word(1), &word(77), domain)
                    .unwrap();
                input
                    .store
                    .write_checked(address(0x102), &word(0), &word(99), domain)
                    .unwrap();
                input
                    .store
                    .write_transient_checked(address(0x101), &word(0), &word(88), domain)
                    .unwrap();
                let expected = input.store.clone();
                let entered = execute_inner(
                    &world,
                    &entry,
                    input,
                    &config,
                    domain,
                    &mut budget,
                    StorageReadPolicy::Abstract,
                    false,
                );
                assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
                let child = entered
                    .successors
                    .into_iter()
                    .find(|next| next.kind == MachineEdgeKind::Call)
                    .unwrap()
                    .payload;
                assert_eq!(child.active().saved_store.state(), &expected);
                let (execution, record) = table.execute(
                    0,
                    &world,
                    &entry,
                    child,
                    &config,
                    domain,
                    &mut budget,
                    &mut counters,
                    StorageReadPolicy::Abstract,
                );
                assert!(record.closed, "{:?}", execution.frontiers);
                let returned = execution
                    .successors
                    .iter()
                    .find(|next| next.kind == MachineEdgeKind::Summary(kind))
                    .unwrap();
                assert_eq!(
                    returned.payload.store, expected,
                    "op={op:02x}, current={current}: the actual rollback must undo entry debit/credit/touches"
                );
            }
        }
    }
}

#[test]
fn a_cached_pure_proof_cannot_hide_effects_of_another_actual_program() {
    use super::super::{StoreEffect, classify};
    use crate::{
        analysis::{FrameCode, PolicyContext},
        bytecode::Program,
    };
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let (world, entry) = fixture(&[
        (0x101, "5f5f5f5f6101025af4"),
        (0x102, "60095f5560075f5d5f5fa000"),
    ]);
    let config = ExecutionConfig {
        max_work: 20_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let mut budget = crate::resource::WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut table = Table::default();
    for current in [0, 1] {
        let mut input = initial(&world, &entry, domain).unwrap();
        input
            .store
            .write_checked(address(0x101), &word(0), &word(current), domain)
            .unwrap();
        input
            .store
            .write_transient_checked(address(0x101), &word(0), &word(88), domain)
            .unwrap();
        let entered = execute_inner(
            &world,
            &entry,
            input,
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
        let child = entered
            .successors
            .into_iter()
            .find(|next| next.kind == MachineEdgeKind::Call)
            .unwrap()
            .payload;
        let (execution, record) = table.execute(
            0,
            &world,
            &entry,
            child,
            &config,
            domain,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        assert!(record.closed, "{:?}", execution.frontiers);
        let returned = execution
            .successors
            .iter()
            .find(|next| next.kind == MachineEdgeKind::Summary(OutcomeKind::Return))
            .unwrap();
        assert_eq!(
            returned
                .payload
                .store
                .read_checked(address(0x101), &word(0), domain)
                .unwrap()
                .singleton(),
            Some(crate::U256::from(9))
        );
        assert_eq!(
            returned
                .payload
                .store
                .read_transient_checked(address(0x101), &word(0), domain)
                .unwrap()
                .singleton(),
            Some(crate::U256::from(7))
        );
        assert!(!returned.payload.store.logs_unknown());
        assert_eq!(returned.payload.store.possible_logs().len(), 1);
        if current == 0 {
            // Model a coarser policy's retained proof independently of its
            // key: the actual invocation below still executes effectful code.
            // A bare cached Unchanged boolean would discard its writes/logs.
            let pure = Program::from_hex("00").unwrap();
            let proof = classify(
                FrameCode::Runtime,
                FrameCode::Runtime,
                Some(&pure),
                false,
                &mut PolicyContext::new(domain, &entry, &mut budget),
            )
            .unwrap();
            assert!(matches!(proof, StoreEffect::Unchanged(_)));
            table.procedures[record.root_procedure.unwrap()].store_effect = proof;
        }
    }
}

#[test]
fn internal_subscription_keeps_its_store_when_a_covering_output_grows_coarser() {
    use crate::analysis::PolicyContext;
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let (world, entry) = fixture(&[(0x101, "5f5f5f5f5f6101025af100"), (0x102, "00")]);
    let config = ExecutionConfig {
        max_work: 20_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let mut budget = crate::resource::WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut table = Table::default();
    let mut input = initial(&world, &entry, domain).unwrap();
    input
        .store
        .write_checked(address(0x101), &word(0), &word(1), domain)
        .unwrap();
    input
        .store
        .write_checked(address(0x101), &word(1), &word(77), domain)
        .unwrap();
    input
        .store
        .write_checked(address(0x102), &word(0), &word(99), domain)
        .unwrap();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let caller = solver.procedure(input).unwrap();
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    let callee = solver.table.subscribers[0].callee;
    assert_eq!(
        solver.table.subscribers[0].from,
        solver.table.procedures[caller].first_body()
    );
    let mut outputs = solver.table.procedures[callee].outputs.clone();
    let output = outputs.get_mut(&OutcomeKind::Return).unwrap();
    output.summary = output
        .summary
        .coarsen(&mut PolicyContext::new(domain, &entry, solver.budget))
        .unwrap();
    output.work = output.summary.work_size();
    output.generation += 1;
    output.updates += 1;
    let work = outputs
        .values()
        .fold(1usize, |work, output| work.saturating_add(output.work));
    solver
        .publish_outputs(callee, outputs, work, false, false)
        .unwrap();
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    let resumed = solver
        .table
        .nodes
        .iter()
        .find(|node| {
            node.procedure == caller
                && node.payload.active().key.basic_block_index == 1
                && node
                    .payload
                    .active()
                    .stack
                    .last()
                    .and_then(|value| value.singleton())
                    == Some(crate::U256::ONE)
        })
        .expect("real successful subscriber delivery admits the caller continuation");
    let retained = resumed.payload.store.instantiate(domain).unwrap();
    for (owner, key, expected) in [(0x101, 0, 1), (0x101, 1, 77), (0x102, 0, 99)] {
        assert_eq!(
            retained
                .read_checked(address(owner), &word(key), domain)
                .unwrap()
                .singleton(),
            Some(crate::U256::from(expected))
        );
    }
}
