//! A local consumer links numeric semantics and bytecode without execution types.

use alloy_primitives::U256;
use analysis_control::{Cancellation, with_cancellation};
use evm_bytecode::{Fork, Program, word};
use evm_domain::{
    DomainSpec, EvmValue, NumericValue, ProductAlgebra, ValueAlgebra, WorkExhausted,
    payment::value_work,
};
use evm_resource::{ResourceError, WorkBudget, pay};
use revm_bytecode::opcode;

#[test]
fn decoded_pure_operations_use_the_same_numeric_evaluation_and_ledger() {
    let program = Program::from_hex_with_fork("600360020300", Fork::Cancun).unwrap();
    let instructions = &program.blocks()[0].instructions;
    let popped = [
        instructions[1].immediate.unwrap(),
        instructions[0].immediate.unwrap(),
    ];
    let op = instructions[2].opcode;
    let expected = word::evaluate(op, &popped).unwrap();
    assert_eq!(expected, U256::MAX);
    let values = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let args = popped.map(EvmValue::<NumericValue>::constant);
    let quote = values.operation_work(op, &args);
    let mut ledger = WorkBudget::new(quote);
    let output = values.apply_budgeted(op, &args, &mut ledger).unwrap();
    assert_eq!(output.value.singleton(), Some(expected));
    assert_eq!(ledger.used(), quote);
    assert!(!ledger.exhausted());
    let mut refused = WorkBudget::new(quote - 1);
    assert_eq!(
        values.apply_budgeted(op, &args, &mut refused).unwrap_err(),
        WorkExhausted
    );
    assert_eq!(refused.used(), 0);
}

#[test]
fn value_quotes_and_payments_retain_both_cancellation_owners() {
    let value = EvmValue::<NumericValue>::constant(U256::ONE);
    let payer = Cancellation::new();
    let mut ledger = with_cancellation(&payer, || WorkBudget::new(100_000));
    let quote = value_work(&value, &ledger).unwrap();
    assert_eq!(quote, value.clone_work());
    assert_eq!(ledger.used(), 0);
    pay(&mut ledger, quote).unwrap();
    assert_eq!(ledger.used(), quote);

    let caller = Cancellation::new();
    caller.cancel();
    assert_eq!(
        with_cancellation(&caller, || value_work(&value, &ledger)),
        Err(ResourceError::Cancelled)
    );
    assert_eq!(
        with_cancellation(&caller, || pay(&mut ledger, 1)),
        Err(ResourceError::Cancelled)
    );
    assert_eq!(ledger.used(), quote);
    payer.cancel();
    assert_eq!(value_work(&value, &ledger), Err(ResourceError::Cancelled));
    assert_eq!(pay(&mut ledger, 1), Err(ResourceError::Cancelled));
    assert_eq!(ledger.used(), quote);
}

#[test]
fn numeric_work_refusal_preserves_operands_and_its_typed_error() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let args = [U256::from(3), U256::MAX].map(EvmValue::<NumericValue>::constant);
    let mut ledger = WorkBudget::new(1);
    assert_eq!(
        values
            .apply_budgeted(opcode::EXP, &args, &mut ledger)
            .unwrap_err(),
        WorkExhausted
    );
    assert_eq!(ledger.used(), 0);
    assert_eq!(args[0].singleton(), Some(U256::from(3)));
    assert_eq!(args[1].singleton(), Some(U256::MAX));
}
