use crate::domain::EvmNumericAlgebra as _;

use super::{byte_work, operation_work};
#[cfg(test)]
use crate::analysis::testing::{Account, ByteArray, Entry, TestValue, World};
use crate::domain::ValueAlgebra;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, InstructionProgress, MachinePayload,
        transfer::{StorageReadPolicy, empty_execution, execute, initial},
    },
    bytecode::Program,
    domain::{
        AlgebraRef, EvmNumericAlgebra, EvmValue, FiniteSetAlgebra, ProductAlgebra,
        provenance::Origin, symbolic::ExprId,
    },
    resource::WorkBudget,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn constant(n: usize) -> TestValue {
    TestValue::constant(U256::from(n))
}

fn fixture() -> (
    World,
    Entry,
    ExecutionConfig,
    crate::domain::ValueAlgebra<ProductAlgebra>,
    MachinePayload,
) {
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
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let algebra =
        crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = algebra.borrowed();
    let mut payload = initial(&world, &entry, domain).unwrap();
    assert_eq!(payload.store.touched_in_transaction(ripemd), Some(false));
    let mut value = TestValue::top().with_expression(ExprId::fresh().unwrap());
    value.set_origin(Origin::Calldata);
    payload.active_mut().stack = vec![value, constant(0)];
    (world, entry, config, algebra, payload)
}

fn assert_per_byte_quote<D: EvmNumericAlgebra>(
    execution: &crate::analysis::transfer::EvmExecution<D::Element>,
    entry: &EvmEntry<D::Element>,
    config: &ExecutionConfig,
    program: &Program,
    offset: EvmValue<D::Element>,
    value: EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
) {
    let frame = execution.payload.active();
    let passes = domain.memory_write_passes(&offset, &value, 32);
    let expected = byte_work(
        &[&offset, &value],
        32,
        frame
            .memory
            .work_size()
            .saturating_add(frame.calldata.work_size())
            .saturating_add(frame.returndata.work_size()),
        domain,
    )
    .saturating_add(passes.saturating_mul((0..32).fold(0usize, |work, index| {
        let literal = U256::from(index);
        let index = EvmValue::constant(literal);
        work.saturating_add(domain.literal_quote_work(literal))
            .saturating_add(domain.operation_work_refs(opcode::BYTE, &[&index, &value]))
    })))
    .saturating_add(passes.saturating_mul(32).saturating_mul(value.clone_work()));
    assert_eq!(
        frame
            .memory
            .uniform_word_write_work(&offset, &value, domain),
        None
    );
    assert_eq!(
        operation_work(
            execution,
            opcode::MSTORE,
            &[offset, value],
            program,
            domain,
            config,
            entry,
        ),
        expected
    );
}

#[test]
fn selected_fee_precedes_touch_and_uncertified_writes_pay_each_byte() {
    let (_, entry, config, algebra, payload) = fixture();
    let domain = algebra.borrowed();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let mut execution = empty_execution(payload);
    let args = [constant(0), execution.payload.active().stack[0].clone()];
    let quote = |execution: &crate::analysis::transfer::Execution, args: &[TestValue], domain| {
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
        .expand(&constant(0), &constant(32), domain)
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
    for (offset, value) in [
        (constant(1), args[1].clone()),
        (
            constant(0),
            TestValue::unknown_byte().with_expression(ExprId::fresh().unwrap()),
        ),
        (constant(0), finite),
    ] {
        assert_per_byte_quote(&execution, &entry, &config, &program, offset, value, domain);
    }

    let finite_algebra = ValueAlgebra::new(FiniteSetAlgebra::from_spec(domain.spec()));
    let finite_domain = finite_algebra.borrowed();
    let owner = Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "finite carrier per-byte quote");
    world
        .insert(owner, EvmAccount::from_hex("525b00", Fork::Osaka).unwrap())
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::repeat_byte(0x22),
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let execution = empty_execution(initial(&world, &entry, finite_domain).unwrap());
    assert_per_byte_quote(
        &execution,
        &entry,
        &config,
        &program,
        EvmValue::constant(U256::ZERO),
        EvmValue::top().with_expression(ExprId::fresh().unwrap()),
        finite_domain,
    );
}

#[test]
fn prepay_refusal_keeps_the_complete_payload_before_any_memory_effect() {
    let (world, entry, config, algebra, payload) = fixture();
    let domain = algebra.borrowed();
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
        &crate::analysis::RecentDistinctJumps,
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
        &crate::analysis::RecentDistinctJumps,
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
