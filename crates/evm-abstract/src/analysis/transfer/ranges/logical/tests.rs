use crate::domain::ValueAlgebra;
use crate::{
    analysis::{
        ChildFrame, Continuation, ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind,
        machine::fixtures::{address, fixture, word},
        transfer::{StorageReadPolicy, WorkBudget, empty_execution, execute, finish, initial},
    },
    domain::{Domain, relational::QueryReason},
    world::{AbstractLog, Account, ByteArray, Existence, LogKey, LogSourceKind},
};

use crate::analysis::transfer::testing::procedure_context as context;

#[test]
fn failure_after_prefix_rolls_back_storage_value_and_logs_but_keeps_address_three_touch() {
    let (world, entry) = fixture(&[(0x101, "00"), (3, "00")]);
    let domain = Domain::default();
    let mut result = empty_execution(initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap());
    let saved = result.payload.active().saved_store.state().clone();
    result.payload.store.touch_account(address(3));
    result
        .payload
        .store
        .write(address(0x101), &word(1), &word(7), domain);
    result
        .payload
        .store
        .write_transient(address(0x101), &word(2), &word(8), domain);
    result.payload.store.write_balance(address(3), word(9));
    let code_hash = result.payload.active().key.code_hash;
    result
        .payload
        .store
        .emit_log(
            LogKey {
                address: address(0x101),
                code_address: address(0x101),
                code_hash,
                source_kind: LogSourceKind::Runtime,
                pc: 1,
            },
            AbstractLog {
                topics: vec![word(3)],
                data: ByteArray::exact(&[0xaa]),
            },
            domain,
        )
        .unwrap();
    let before = result.payload.clone();
    let config = ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    assert!(super::super::memory(
        &mut result,
        &word(0),
        &word(1),
        &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
        9
    ));
    assert_eq!(
        result.payload, before,
        "the memory failure is emitted before this instruction's effect"
    );
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    let failed = &result.outcomes[0];
    assert_eq!(failed.kind, OutcomeKind::Failure);
    assert_eq!(
        failed.store.read(address(0x101), &word(1), domain),
        saved.read(address(0x101), &word(1), domain)
    );
    assert_eq!(
        failed
            .store
            .read_transient(address(0x101), &word(2), domain),
        saved.read_transient(address(0x101), &word(2), domain)
    );
    assert_eq!(
        failed.store.read_balance(address(3)),
        saved.read_balance(address(3))
    );
    assert_eq!(failed.store.possible_logs(), saved.possible_logs());
    assert_eq!(failed.store.touched_in_transaction(address(3)), Some(true));
    assert_eq!(saved.touched_in_transaction(address(3)), Some(false));
}

#[test]
fn mcopy_has_one_pre_effect_failure_and_resumes_the_childs_parent_with_empty_data() {
    let (world, entry) = fixture(&[(0x101, "00"), (0x200, "00"), (3, "00")]);
    let domain = Domain::default();
    let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    payload
        .active_mut()
        .memory
        .write_word(&word(0), &word(0x1234), 64, domain)
        .unwrap();
    payload
        .store
        .write(address(0x101), &word(1), &word(42), domain);
    let parent_memory = payload.active().memory.clone();
    let saved = payload.store.snapshot();
    let mut child = payload.active().clone();
    child.key.address = address(0x200);
    child.key.code_address = address(0x200);
    child.key.address_value = address(0x200).into();
    child.memory = ByteArray::memory();
    child.returndata = ByteArray::exact(&[0xaa]);
    child.stack.clear();
    child.saved_store = saved.clone();
    payload.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: Some(1),
            output_offset: word(0),
            output_size: word(32),
            creation: None,
        },
    });
    payload.store.touch_account(address(3));
    payload
        .store
        .write(address(0x200), &word(1), &word(99), domain);
    payload.store.write_balance(address(0x200), word(1));
    let before = payload.clone();
    let mut result = empty_execution(payload);
    let config = ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    assert!(super::super::memory_copy(
        &mut result,
        &word(32),
        &word(0),
        &word(7),
        &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
        8
    ));
    assert_eq!(result.payload, before);
    assert!(result.frontiers.is_empty());
    assert_eq!(
        result.successors.len(),
        1,
        "one failure covers either MCOPY endpoint"
    );
    let failed = &result.successors[0];
    assert_eq!(failed.kind, MachineEdgeKind::Failure);
    assert_eq!(failed.payload.call_stack.depth(), 1);
    assert_eq!(
        failed.payload.active().stack.last().unwrap().singleton(),
        Some(crate::U256::ZERO)
    );
    assert_eq!(
        failed.payload.active().returndata.len().singleton(),
        Some(crate::U256::ZERO)
    );
    assert_eq!(failed.payload.active().memory, parent_memory);
    assert_eq!(failed.payload.active().key.basic_block_index, 1);
    assert_eq!(
        failed
            .payload
            .store
            .read(address(0x101), &word(1), domain)
            .singleton(),
        Some(crate::U256::from(42))
    );
    assert_eq!(
        failed.payload.store.read(address(0x200), &word(1), domain),
        saved.state().read(address(0x200), &word(1), domain)
    );
    assert_eq!(
        failed.payload.store.read_balance(address(0x200)),
        saved.state().read_balance(address(0x200))
    );
    assert_eq!(
        failed.payload.store.touched_in_transaction(address(3)),
        Some(true)
    );
    assert_eq!(result.completed_calls.len(), 1);
    assert_eq!(
        result.completed_calls[0].payload.active().memory,
        before.active().memory
    );
}

#[test]
fn rejected_work_and_cancellation_leave_the_pre_instruction_state_intact() {
    for cancelled in [false, true] {
        let (world, entry) = fixture(&[(0x101, "00")]);
        let mut result = empty_execution(
            initial(&world, &entry, ValueAlgebra::new(&Domain::default())).unwrap(),
        );
        let before = result.payload.clone();
        let cancellation = analysis_control::Cancellation::new();
        if cancelled {
            cancellation.cancel();
        }
        let config = ExecutionConfig::default();
        analysis_control::with_cancellation(&cancellation, || {
            let mut budget = WorkBudget::new(if cancelled { config.max_work } else { 0 });
            assert!(!super::super::memory(
                &mut result,
                &word(0),
                &word(7),
                &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
                4
            ));
        });
        assert_eq!(result.payload, before);
        assert!(result.outcomes.is_empty());
        assert!(result.successors.is_empty());
        assert_eq!(result.frontiers.len(), 1);
        assert_eq!(
            result.frontiers[0].reason,
            if cancelled {
                FrontierReason::Relations(QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            }
        );
    }
}

#[test]
fn large_deployed_code_first_mstore_keeps_failure_and_success_at_default_budget() {
    for (bytes, helper_bytes) in [(3_124, 0), (23_464, 2_186)] {
        // The observed deployed-code prefix executes its first MSTORE at pc 4.
        // Padding is unreachable after STOP; it only grows shared runtime code.
        let code = format!("608060405200{}", "00".repeat(bytes - 6));
        let helper = "00".repeat(helper_bytes);
        let accounts = if helper_bytes == 0 {
            vec![(0x101, code.as_str())]
        } else {
            vec![(0x101, code.as_str()), (0x200, helper.as_str())]
        };
        let (world, entry) = fixture(&accounts);
        let config = ExecutionConfig::default();
        assert_eq!(config.max_work, 20_000_000);
        let domain = config.domain().unwrap();
        let input = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        assert_eq!(input.store.addresses().len(), accounts.len());
        let mut budget = WorkBudget::new(config.max_work);
        let result = execute(
            &world,
            &entry,
            input.clone(),
            &config,
            ValueAlgebra::new(&domain),
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
        assert_eq!(result.executed_pcs, [0, 2, 4, 5]);
        assert_eq!(result.outcomes.len(), 2);
        assert!(
            result
                .outcomes
                .iter()
                .any(|o| o.kind == OutcomeKind::Return)
        );
        let failed = result
            .outcomes
            .iter()
            .find(|o| o.kind == OutcomeKind::Failure)
            .unwrap();
        assert_eq!(
            failed.store.code(address(0x101)),
            input.store.code(address(0x101))
        );
        assert_eq!(failed.data.len().singleton(), Some(crate::U256::ZERO));
        assert_eq!(
            result
                .payload
                .active()
                .memory
                .read_word_checked(&word(64), domain)
                .unwrap()
                .singleton(),
            Some(crate::U256::from(128))
        );
        assert!(budget.used() < config.max_work);
    }
}

#[test]
fn root_return_revert_and_failure_plan_the_correct_prefix_and_sticky_touch() {
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        let (mut world, entry) = fixture(&[(0x101, "00")]);
        world.insert(address(3), Account::empty()).unwrap();
        world.insert(address(0x200), Account::empty()).unwrap();
        let domain = Domain::default();
        let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        let saved = payload.active().saved_store.clone();
        payload.store.touch_account(address(3));
        payload.store.touch_account(address(0x200));
        payload.store.write_balance(address(3), word(9));
        payload
            .store
            .write(address(0x101), &word(1), &word(7), domain);
        payload
            .store
            .write_transient(address(0x101), &word(2), &word(8), domain);
        let code_hash = payload.active().key.code_hash;
        payload
            .store
            .emit_log(
                LogKey {
                    address: address(0x101),
                    code_address: address(0x101),
                    code_hash,
                    source_kind: LogSourceKind::Runtime,
                    pc: 1,
                },
                AbstractLog {
                    topics: vec![word(3)],
                    data: ByteArray::exact(&[0xaa]),
                },
                domain,
            )
            .unwrap();
        let data = if kind == OutcomeKind::Failure {
            ByteArray::empty()
        } else {
            ByteArray::exact(&[0xbb])
        };
        let config = ExecutionConfig::default();
        let mut budget = WorkBudget::new(config.max_work);
        let transfer_algebra = ValueAlgebra::new(Domain::default());
        let mut transfer = context(&config, &mut budget, transfer_algebra.borrowed());
        transfer.procedure = false;
        let mut result = empty_execution(payload.clone());
        finish(
            &mut result,
            payload.clone(),
            kind,
            data.clone(),
            &mut transfer,
            9,
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(result.payload, payload);
        assert_eq!(result.outcomes.len(), 1);
        let outcome = &result.outcomes[0];
        assert_eq!(outcome.kind, kind);
        assert_eq!(outcome.data, data);
        if kind == OutcomeKind::Return {
            assert_eq!(outcome.store.existence(address(3)), Existence::Present);
            assert_eq!(outcome.store.read_balance(address(3)), word(9));
            assert_eq!(outcome.store.existence(address(0x200)), Existence::Absent);
            assert_eq!(
                outcome.store.read(address(0x101), &word(1), domain),
                word(7)
            );
            assert_eq!(outcome.store.possible_logs().len(), 1);
        } else {
            assert_eq!(outcome.store.existence(address(3)), Existence::Absent);
            assert_eq!(outcome.store.existence(address(0x200)), Existence::Present);
            assert_eq!(
                outcome.store.read(address(0x101), &word(1), domain),
                saved.state().read(address(0x101), &word(1), domain)
            );
            assert!(outcome.store.possible_logs().is_empty());
        }
        assert_eq!(
            outcome
                .store
                .read_transient(address(0x101), &word(2), domain),
            word(0)
        );
        assert_eq!(
            outcome.store.touched_in_transaction(address(3)),
            Some(false)
        );

        // Refuse the execution reservation after staging/rollback/planning.
        // No semantic outcome or closed lifecycle flags may escape that scratch.
        for cancelled in [false, true] {
            let cancellation = analysis_control::Cancellation::new();
            let mut refused = empty_execution(payload.clone());
            analysis_control::with_cancellation(&cancellation, || {
                let mut small = WorkBudget::new(if cancelled {
                    config.max_work
                } else {
                    budget.used() - 1
                });
                if cancelled {
                    cancellation.cancel();
                }
                let transfer_algebra = ValueAlgebra::new(Domain::default());
                let mut transfer = context(&config, &mut small, transfer_algebra.borrowed());
                transfer.procedure = false;
                finish(
                    &mut refused,
                    payload.clone(),
                    kind,
                    data.clone(),
                    &mut transfer,
                    9,
                );
            });
            assert_eq!(refused.payload, payload);
            assert!(refused.outcomes.is_empty());
            assert_eq!(refused.frontiers.len(), 1);
            assert_eq!(
                refused.frontiers[0].reason,
                if cancelled {
                    FrontierReason::Relations(QueryReason::Cancelled)
                } else {
                    FrontierReason::Work
                }
            );
        }
    }
}

#[test]
fn root_may_delete_retention_refusal_emits_a_typed_frontier_without_an_outcome() {
    let (mut world, entry) = fixture(&[(0x101, "00")]);
    let mut config = ExecutionConfig::default();
    // Keep the source-token + input root valid; refuse only its deletion ITE.
    config.analysis.relations.max_nodes = 2;
    let domain = config.domain().unwrap();
    world
        .insert(
            address(0x200),
            Account {
                code: crate::world::Code::Empty,
                nonce: word(0),
                balance: domain.join(&word(0), &word(1)),
                existence: Existence::Present,
                ..Account::unknown()
            },
        )
        .unwrap();
    let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    payload.store.touch_account(address(0x200));
    payload
        .store
        .check_existing(domain.spec().relations().expressions())
        .unwrap();
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(config.max_work);
    let transfer_algebra = ValueAlgebra::new(Domain::default());
    let mut transfer = context(&config, &mut budget, transfer_algebra.borrowed());
    transfer.domain = ValueAlgebra::new(&domain);
    transfer.procedure = false;
    finish(
        &mut result,
        payload.clone(),
        OutcomeKind::Return,
        ByteArray::empty(),
        &mut transfer,
        0,
    );
    assert_eq!(result.payload, payload);
    assert!(result.outcomes.is_empty());
    assert!(result.successors.is_empty());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(
        result.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
    assert!(!budget.exhausted());
}

#[test]
fn literal_boundary_keeps_pre_effect_gas_failure_and_live_payload_on_either_decision() {
    use crate::{U256, domain::AbstractValue};

    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    let saved = payload.active().saved_store.state().clone();
    payload
        .active_mut()
        .memory
        .write_word(&word(0), &word(0x1234), config.max_memory_bytes, domain)
        .unwrap();
    payload
        .store
        .write(address(0x101), &word(1), &word(99), domain);
    payload.store.write_balance(address(0x101), word(7));
    let bound = super::MAX_END;
    let mut covered = empty_execution(payload.clone());
    let mut gas_budget = WorkBudget::new(config.max_work);
    let transfer_algebra = ValueAlgebra::new(Domain::default());
    let mut transfer = context(&config, &mut gas_budget, transfer_algebra.borrowed());
    transfer.procedure = false;
    assert!(super::failure(&mut covered, &mut transfer, 9));
    let gas_work = gas_budget.used();
    for (offset, size, allowed) in [
        (U256::ZERO, bound, true),
        (bound - U256::ONE, U256::ONE, true),
        (bound - U256::ONE, U256::from(2), false),
        (bound, U256::ONE, false),
        (U256::MAX - U256::ONE, U256::from(2), false),
        (U256::MAX, U256::ONE, false),
        (U256::MAX, U256::ZERO, true),
    ] {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        let transfer_algebra = ValueAlgebra::new(Domain::default());
        let mut transfer = context(&config, &mut budget, transfer_algebra.borrowed());
        transfer.procedure = false;
        assert_eq!(
            super::super::memory(
                &mut result,
                &AbstractValue::constant(offset),
                &AbstractValue::constant(size),
                &mut transfer,
                9,
            ),
            allowed
        );
        assert_eq!(result.payload, payload);
        assert_eq!(
            budget.used(),
            gas_work + if size == U256::ZERO { 736 } else { 1_600 }
        );
        assert!(!budget.exhausted());
        assert!(result.frontiers.is_empty());
        assert!(result.diagnostics.is_empty());
        assert!(result.successors.is_empty());
        assert!(result.completed_calls.is_empty());
        assert_eq!(result.outcomes.len(), 1);
        let failed = &result.outcomes[0];
        assert_eq!(failed.kind, OutcomeKind::Failure);
        assert_eq!(failed.data, ByteArray::empty());
        assert_eq!(
            failed.store.read(address(0x101), &word(1), domain),
            saved.read(address(0x101), &word(1), domain)
        );
        assert_eq!(
            failed.store.read_balance(address(0x101)),
            saved.read_balance(address(0x101))
        );
    }
}

#[test]
fn preflight_refusal_preserves_the_already_emitted_gas_failure() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let payload = initial(&world, &entry, ValueAlgebra::new(&Domain::default())).unwrap();
    let mut covered = empty_execution(payload.clone());
    let mut full = WorkBudget::new(config.max_work);
    assert!(super::failure(
        &mut covered,
        &mut context(&config, &mut full, ValueAlgebra::new(&Domain::default())),
        9
    ));
    assert!(covered.frontiers.is_empty());
    assert_eq!(covered.outcomes.len(), 1);
    let gas_work = full.used();
    for cancelled in [false, true] {
        let cancellation = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&cancellation, || {
            let mut result = empty_execution(payload.clone());
            let mut budget = WorkBudget::new(if cancelled {
                config.max_work
            } else {
                gas_work + super::super::LITERAL_MEMORY_METADATA_WORK + 608 - 1
            });
            if cancelled {
                assert!(super::failure(
                    &mut result,
                    &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
                    9,
                ));
                cancellation.cancel();
                assert!(!super::super::allow_memory(
                    &mut result,
                    &word(0),
                    &word(0),
                    &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
                    9,
                ));
            } else {
                assert!(!super::super::memory(
                    &mut result,
                    &word(0),
                    &word(0),
                    &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
                    9,
                ));
            }
            assert_eq!(
                budget.used(),
                gas_work
                    + if cancelled {
                        0
                    } else {
                        super::super::LITERAL_MEMORY_METADATA_WORK
                    }
            );
            assert!(budget.exhausted());
            assert_eq!(result.payload, payload);
            assert!(result.successors.is_empty());
            assert!(result.completed_calls.is_empty());
            assert_eq!(result.outcomes.len(), 1);
            assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
            assert_eq!(result.outcomes[0].data, ByteArray::empty());
            assert_eq!(result.outcomes[0].store, covered.outcomes[0].store);
            assert_eq!(result.frontiers.len(), 1);
            assert_eq!(
                result.frontiers[0].reason,
                if cancelled {
                    FrontierReason::Relations(QueryReason::Cancelled)
                } else {
                    FrontierReason::Work
                }
            );
        });
    }
}

#[test]
fn nonwrapping_literal_return_range_stops_without_a_return_or_successor() {
    use crate::{U256, analysis::InstructionProgress, domain::AbstractValue};

    let (world, entry) = fixture(&[(0x101, "f3")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    for (offset, size) in [
        (super::MAX_END - U256::ONE, U256::from(2)),
        (U256::MAX - U256::ONE, U256::from(2)),
        (U256::MAX, U256::ONE),
    ] {
        let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        let saved = payload.active().saved_store.state().clone();
        payload
            .store
            .write(address(0x101), &word(1), &word(99), domain);
        payload.active_mut().stack = vec![
            AbstractValue::constant(size),
            AbstractValue::constant(offset),
        ];
        let before = payload.clone();
        let mut budget = WorkBudget::new(config.max_work);
        let result = execute(
            &world,
            &entry,
            payload,
            &config,
            ValueAlgebra::new(&domain),
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(!budget.exhausted());
        assert_eq!(result.executed_pcs, [0]);
        assert_eq!(
            result.instruction_progress,
            [InstructionProgress::OperandsConsumed]
        );
        assert!(result.payload.active().stack.is_empty());
        assert_eq!(result.payload.active().memory, before.active().memory);
        assert_eq!(result.payload.store, before.store);
        assert!(result.frontiers.is_empty());
        assert!(result.successors.is_empty());
        assert!(result.completed_calls.is_empty());
        assert_eq!(result.outcomes.len(), 1);
        assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
        assert_eq!(result.outcomes[0].data, ByteArray::empty());
        assert_eq!(
            result.outcomes[0]
                .store
                .read(address(0x101), &word(1), domain),
            saved.read(address(0x101), &word(1), domain)
        );
    }
}

#[test]
fn zero_length_return_does_not_clear_an_inherited_literal_expression_limit() {
    let (world, entry) = fixture(&[(0x101, "f3")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    payload.active_mut().stack = vec![word(0), word(0).with_symbolic_limit()];
    let mut budget = WorkBudget::new(config.max_work);
    let result = execute(
        &world,
        &entry,
        payload,
        &config,
        ValueAlgebra::new(&domain),
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert!(!budget.exhausted());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].pc, 0);
    assert_eq!(
        result.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
    assert_eq!(
        result
            .outcomes
            .iter()
            .map(|outcome| outcome.kind)
            .collect::<Vec<_>>(),
        [OutcomeKind::Failure, OutcomeKind::Return]
    );
}

#[test]
fn empty_returndata_copy_still_checks_the_original_source_bound() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    for (offset, allowed) in [
        (word(0), true),
        (word(1), false),
        (
            crate::domain::AbstractValue::constant(crate::U256::MAX),
            false,
        ),
    ] {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        assert_eq!(
            super::super::returndata(
                &mut result,
                &offset,
                &word(0),
                &word(0),
                &mut context(&config, &mut budget, ValueAlgebra::new(&Domain::default())),
                9,
            ),
            allowed
        );
        assert_eq!(result.payload, payload);
        assert!(result.frontiers.is_empty());
        assert!(result.successors.is_empty());
        assert_eq!(result.outcomes.len(), usize::from(!allowed));
        if !allowed {
            assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
            assert_eq!(result.outcomes[0].data, ByteArray::empty());
        }
    }
}

#[test]
fn zero_size_literal_does_not_bypass_an_invalid_root_in_failure_completion() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let payload = initial(&world, &entry, ValueAlgebra::new(&Domain::default())).unwrap();
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 1;
    let domain = config.domain().unwrap();
    // Source token + captured input root needs two nodes. The normal range is
    // empty, but its preceding root rollback/finalization still owns this check.
    assert!(
        payload
            .store
            .check_existing(domain.spec().relations().expressions())
            .is_err()
    );
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(config.max_work);
    let transfer_algebra = ValueAlgebra::new(Domain::default());
    let mut transfer = context(&config, &mut budget, transfer_algebra.borrowed());
    transfer.domain = ValueAlgebra::new(&domain);
    transfer.procedure = false;
    assert!(!super::super::memory(
        &mut result,
        &crate::domain::AbstractValue::constant(crate::U256::MAX),
        &word(0),
        &mut transfer,
        9,
    ));
    assert_eq!(result.payload, payload);
    assert!(!budget.exhausted());
    assert!(result.outcomes.is_empty());
    assert!(result.successors.is_empty());
    assert!(result.completed_calls.is_empty());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(
        result.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
}
