//! Early known entries retain the real callee and caller completion contract.

use super::super::{is_known_call_entry, is_recursive_entry};
use crate::{
    analysis::{
        ExecutionConfig, FrameCode, FrontierReason, MachineEdgeKind, MachinePayload, OutcomeKind,
        ProcedureSummaryKind,
        engine::Counters,
        machine::fixtures::{address, fixture, word},
        testing::{ByteArray, Entry, TestValue, World},
        transfer::{
            StorageReadPolicy,
            procedure::{
                Table,
                testing::{execute_inner, initial},
            },
        },
    },
    domain::{AlgebraRef, ProductAlgebra},
    resource::WorkBudget,
    world::AddressInput,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn call_fixture(op: u8, callee: &str) -> (World, Entry) {
    let args = if matches!(op, opcode::CALL | opcode::CALLCODE) {
        "5f5f5f5f5f"
    } else {
        "5f5f5f5f"
    };
    // End immediately after the CALL-family opcode, without an explicit STOP.
    let code = format!("{args}6101025a{op:02x}");
    let (world, _) = fixture(&[(0x101, &code), (0x102, callee)]);
    (world, Entry::new(address(0x101)))
}

fn first_call(
    world: &World,
    entry: &Entry,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, ProductAlgebra>,
    budget: &mut WorkBudget,
) -> MachinePayload {
    let input = initial(world, entry, domain).unwrap();
    assert!(!is_known_call_entry(&input), "a root is not a CALL child");
    let execution = execute_inner(
        world,
        entry,
        input,
        config,
        domain,
        budget,
        StorageReadPolicy::Abstract,
        false,
    );
    assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
    execution
        .successors
        .into_iter()
        .find(|successor| successor.kind == MachineEdgeKind::Call)
        .expect("the known runtime child is entered")
        .payload
}

#[test]
fn call_family_entries_keep_end_of_code_owner_and_static_contexts() {
    let algebra = crate::analysis::testing::product(Default::default());
    let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    for op in [
        opcode::CALL,
        opcode::CALLCODE,
        opcode::DELEGATECALL,
        opcode::STATICCALL,
    ] {
        for is_static in [false, true] {
            let (world, mut entry) = call_fixture(op, "00");
            entry.environment.is_static = is_static;
            let mut budget = WorkBudget::new(config.max_work);
            let input = first_call(&world, &entry, &config, domain, &mut budget);
            assert!(is_known_call_entry(&input), "op={op:02x}");
            assert!(!is_recursive_entry(&input));
            let child = input.call_stack.active_child().unwrap();
            let parent = input.call_stack.parent().unwrap();
            assert_eq!(
                child.continuation.return_block,
                Some(parent.program.as_ref().unwrap().blocks().len()),
                "the final CALL resumes at the synthetic end block"
            );
            assert_eq!(child.state.key.code_address, address(0x102));
            let owner = if matches!(op, opcode::CALLCODE | opcode::DELEGATECALL) {
                address(0x101)
            } else {
                address(0x102)
            };
            assert_eq!(child.state.key.address, owner);
            assert_eq!(child.state.key.address_value, owner.into());
            assert_eq!(
                child.state.key.is_static,
                is_static || op == opcode::STATICCALL
            );
            if op == opcode::DELEGATECALL {
                assert_eq!(child.state.key.caller, parent.key.caller);
                assert_eq!(child.state.call_value, parent.call_value);
            }
        }
    }
}

#[test]
fn eligibility_depends_on_entry_shape_not_call_inputs_or_return_destination() {
    let algebra = crate::analysis::testing::product(Default::default());
    let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    let (world, entry) = call_fixture(opcode::CALL, "00");
    let mut budget = WorkBudget::new(config.max_work);
    let mut input = first_call(&world, &entry, &config, domain, &mut budget);
    let child = input.call_stack.children_mut().last_mut().unwrap();
    child.state.key.caller = AddressInput::unknown_caller();
    child.state.caller_operand = Some(TestValue::unknown_address());
    child.state.call_value = TestValue::top();
    child.state.calldata = ByteArray::unknown();
    child.state.key.address_value = AddressInput::unknown_to();
    child.state.address_operand = Some(TestValue::unknown_address());
    child.continuation.output_offset = TestValue::top();
    child.continuation.output_size = TestValue::top();
    // Admission does not interpret the caller's return destination. Current
    // production CALLs use Some(block_count) for end of code, tested above.
    child.continuation.return_block = None;
    assert!(is_known_call_entry(&input));
    for variant in 0..6 {
        let mut rejected = input.clone();
        let child = rejected.call_stack.children_mut().last_mut().unwrap();
        match variant {
            0 => child.continuation.creation = Some(address(0x103)),
            1 => child.state.code = FrameCode::InitCode,
            2 => child.state.key.mode = FrameCode::InitCode,
            3 => child.state.program = None,
            4 => child.state.key.basic_block_index = 1,
            5 => child.state.stack.push(word(1)),
            _ => unreachable!(),
        }
        assert!(!is_known_call_entry(&rejected), "variant={variant}");
    }
    for code in [
        FrameCode::Unknown,
        FrameCode::Empty,
        FrameCode::Precompile(address(1)),
        FrameCode::InvalidDelegation,
    ] {
        let mut rejected = input.clone();
        rejected.active_mut().code = code;
        rejected.active_mut().key.mode = code;
        assert!(!is_known_call_entry(&rejected), "code={code:?}");
    }
}

#[test]
fn early_leaf_is_known_call_and_resumes_the_final_callers_empty_block() {
    let algebra = crate::analysis::testing::product(Default::default());
    let domain = algebra.borrowed();
    let (world, entry) = call_fixture(opcode::CALL, "00");
    for use_summaries in [false, true] {
        let config = ExecutionConfig {
            use_summaries,
            ..ExecutionConfig::default()
        };
        let mut budget = WorkBudget::new(config.max_work);
        let input = first_call(&world, &entry, &config, domain, &mut budget);
        let next = input
            .call_stack
            .active_child()
            .unwrap()
            .continuation
            .return_block;
        let mut table = Table::default();
        let mut counters = Counters::default();
        let (execution, record) = table.execute(
            0,
            &world,
            &entry,
            input,
            &config,
            domain,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(record.kind, ProcedureSummaryKind::KnownCall);
        assert!(record.closed, "{:?}", execution.frontiers);
        let root = record.root_procedure.unwrap();
        let retained = &table.procedures[root];
        assert_eq!(retained.namespace, usize::from(!use_summaries));
        assert_eq!(retained.entry_payload.frames.len(), 1);
        assert!(retained.entry_payload.continuations.is_empty());
        assert_eq!(record.procedures[root].frame.code_address, address(0x102));
        assert!(!retained.recursive);
        assert!(execution.outcomes.is_empty());
        assert_eq!(execution.completed_calls.len(), 1);
        let returned = execution
            .successors
            .into_iter()
            .find(|successor| successor.kind == MachineEdgeKind::Summary(OutcomeKind::Return))
            .expect("callee completion resumes the actual caller");
        assert_eq!(returned.payload.call_stack.depth(), 1);
        assert_eq!(Some(returned.payload.active().key.basic_block_index), next);
        assert_eq!(
            returned.payload.active().stack.last().unwrap().singleton(),
            Some(U256::ONE)
        );
        let finished = execute_inner(
            &world,
            &entry,
            returned.payload,
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert!(finished.frontiers.is_empty());
        assert!(finished.successors.is_empty());
        assert!(
            finished
                .outcomes
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return)
        );
    }
}

#[test]
fn early_known_loop_cannot_close_an_empty_output_relation() {
    let algebra = crate::analysis::testing::product(Default::default());
    let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    let (world, entry) = call_fixture(opcode::CALL, "5b5f56");
    let mut budget = WorkBudget::new(config.max_work);
    let input = first_call(&world, &entry, &config, domain, &mut budget);
    assert!(is_known_call_entry(&input));
    assert!(!is_recursive_entry(&input));
    let mut table = Table::default();
    let mut counters = Counters::default();
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert_eq!(record.kind, ProcedureSummaryKind::KnownCall);
    assert!(!record.closed);
    let root = record.root_procedure.unwrap();
    assert!(table.procedures[root].local_cycle);
    assert!(!table.procedures[root].recursive);
    assert_eq!(record.procedures[root].covered_blocks, [0]);
    assert!(record.procedures[root].outputs.is_empty());
    assert!(execution.outcomes.is_empty());
    assert!(execution.successors.is_empty());
    assert_eq!(execution.frontiers.len(), 1);
    assert_eq!(
        execution.frontiers[0].reason,
        FrontierReason::RecursiveNoTerminal
    );
    let pending = execution.frontiers[0].payload.as_ref().unwrap();
    assert_eq!(pending.active().key.code_address, address(0x102));
    assert_eq!(pending.call_stack.depth(), 1);
}
