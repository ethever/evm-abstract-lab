use super::{byte_work, operation_work};
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, InstructionProgress, MachinePayload,
        transfer::{StorageReadPolicy, empty_execution, execute, initial},
    },
    bytecode::Program,
    domain::{AbstractValue, Domain, provenance::Origin, symbolic::ExprId},
    resource::WorkBudget,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn constant(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn fixture() -> (World, Entry, ExecutionConfig, Domain, MachinePayload) {
    let owner = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "A1 prepayment");
    world
        .insert(owner, Account::from_hex("525b00", Fork::Osaka).unwrap())
        .unwrap();
    let ripemd = Address::from_word(U256::from(3).into());
    world.insert(ripemd, Account::empty()).unwrap();
    let entry = Entry::concrete(
        owner,
        Address::repeat_byte(0x22),
        constant(0),
        ByteArray::empty(),
    );
    let mut config = ExecutionConfig::default();
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    let mut payload = initial(&world, &entry, domain).unwrap();
    assert_eq!(payload.store.touched_in_transaction(ripemd), Some(false));
    let mut value = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    value.set_origin(Origin::Calldata);
    payload.active_mut().stack = vec![value, constant(0)];
    (world, entry, config, domain, payload)
}

#[test]
fn selected_fee_precedes_touch_and_noneligible_quotes_are_exactly_original() {
    let (_, entry, config, domain, payload) = fixture();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let mut execution = empty_execution(payload);
    let args = [constant(0), execution.payload.active().stack[0].clone()];
    let quote = |execution: &super::super::Execution, args: &[AbstractValue], domain| {
        operation_work(
            execution,
            opcode::MSTORE,
            args,
            &program,
            domain,
            &config,
            &entry,
        )
    };
    let memory = &execution.payload.active().memory;
    assert_eq!(
        quote(&execution, &args, domain),
        memory
            .uniform_word_write_work(&args[0], &args[1], domain)
            .unwrap()
    );
    execution
        .payload
        .active_mut()
        .memory
        .expand(&constant(0), &constant(32), config.max_memory_bytes, domain)
        .unwrap();
    assert_eq!(
        quote(&execution, &args, domain),
        execution
            .payload
            .active()
            .memory
            .uniform_word_write_work(&args[0], &args[1], domain)
            .unwrap()
    );

    let finite = domain.join(&constant(1), &constant(2));
    let constants = Domain::new(std::num::NonZeroUsize::new(8).unwrap());
    for (offset, value, selected) in [
        (constant(1), args[1].clone(), domain),
        (
            constant(0),
            AbstractValue::unknown_byte().with_expression(ExprId::fresh().unwrap()),
            domain,
        ),
        (constant(0), finite, domain),
        (constant(0), args[1].clone(), constants),
    ] {
        let frame = execution.payload.active();
        let original = byte_work(
            &[&offset, &value],
            32,
            frame
                .memory
                .work_size()
                .saturating_add(frame.calldata.work_size())
                .saturating_add(frame.returndata.work_size()),
            selected,
        )
        .saturating_add(
            32usize.saturating_mul(selected.operation_work(&[constant(0), value.clone()])),
        );
        assert_eq!(
            frame
                .memory
                .uniform_word_write_work(&offset, &value, selected),
            None
        );
        assert_eq!(quote(&execution, &[offset, value], selected), original);
    }
}

#[test]
fn prepay_refusal_keeps_the_complete_payload_before_any_memory_effect() {
    let (world, entry, config, domain, payload) = fixture();
    let args = [constant(0), payload.active().stack[0].clone()];
    let fee = payload
        .active()
        .memory
        .uniform_word_write_work(&args[0], &args[1], domain)
        .unwrap();
    let mut expected = payload.clone();
    expected.active_mut().stack.clear();
    // Entry/instruction costs leave less than fee at the operation gate. The
    // gate must decline before touch_memory can install its first allocation.
    let mut budget = WorkBudget::new(fee);
    let result = execute(
        &world,
        &entry,
        payload.clone(),
        &config,
        domain,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert_eq!(result.executed_pcs, [0]);
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::OperandsConsumed]
    );
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(result.payload, expected);
    assert!(result.successors.is_empty() && result.outcomes.is_empty());
    assert!(budget.used() > 0 && budget.used() < fee);
    assert!(budget.exhausted());

    // Same input and policy with the ordinary root ledger reaches the write.
    let mut budget = WorkBudget::new(config.max_work);
    let control = execute(
        &world,
        &entry,
        payload.clone(),
        &config,
        domain,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    assert!(control.frontiers.is_empty(), "{:?}", control.frontiers);
    assert_eq!(
        control.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert_eq!(control.successors.len(), 1);
    assert_eq!(control.payload.active().memory.stored_bytes().len(), 32);
    assert_eq!(
        control.payload.active().saved_store,
        payload.active().saved_store
    );
    assert_eq!(payload.active().memory, ByteArray::memory());
}
