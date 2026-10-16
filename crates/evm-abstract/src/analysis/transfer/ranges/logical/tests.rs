use crate::{
    analysis::{
        ChildFrame, Continuation, ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind,
        machine::fixtures::{address, fixture, word},
        transfer::{TransferContext, WorkBudget, empty_execution, initial},
    },
    domain::{Domain, relational::QueryReason},
    world::{AbstractLog, ByteArray, LogKey, LogSourceKind},
};

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: true,
        discover_code: false,
    }
}

#[test]
fn failure_after_prefix_rolls_back_storage_value_and_logs_but_keeps_address_three_touch() {
    let (world, entry) = fixture(&[(0x101, "00"), (3, "00")]);
    let domain = Domain::default();
    let mut result = empty_execution(initial(&world, &entry, domain).unwrap());
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
        &mut context(&config, &mut budget),
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
    let mut payload = initial(&world, &entry, domain).unwrap();
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
        &mut context(&config, &mut budget),
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
        let mut result = empty_execution(initial(&world, &entry, Domain::default()).unwrap());
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
                &mut context(&config, &mut budget),
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
