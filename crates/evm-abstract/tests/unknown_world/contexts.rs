use super::{fixtures, oracle, support::*};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::world::{EvmAccount, EvmByteArray};
use revm_bytecode::opcode;

#[test]
fn unknown_delegatecall_can_write_current_storage_without_callback() {
    check_direct_storage(opcode::DELEGATECALL);
}

#[test]
fn unknown_callcode_can_write_current_storage_without_callback() {
    check_direct_storage(opcode::CALLCODE);
}

fn check_direct_storage(op: u8) {
    let a = fixtures::preserved_parent(op, false);
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    let analysis = run(world, entry(EvmByteArray::<NumericValue>::empty()));
    complete_unknown(&analysis);
    let outcomes = returns(&analysis);
    for sample in [0, 9, 42, 256, 65535] {
        assert!(
            outcomes
                .iter()
                .any(|outcome| output_word(outcome, 0).contains(U256::ONE)
                    && output_word(outcome, 32).contains(U256::from(sample))
                    && read(&outcome.store, A, 0).contains(U256::from(sample))),
            "{op:#x} unknown code cannot write owner slot to {sample}"
        );
    }
}

pub(super) fn readonly_parent(op: u8) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .call(op, U as u16, 0, (0, 0), (0, 0))
        .mstore(0)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .return_range(0, 64);
    a.label("callback").op(opcode::STOP);
    a.finish()
}

#[test]
fn unknown_staticcall_preserves_known_storage_even_across_callbacks() {
    let a = readonly_parent(opcode::STATICCALL);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 42)]);
    let analysis = run(world, entry(EvmByteArray::<NumericValue>::empty()));
    complete_unknown(&analysis);
    assert!(
        returns(&analysis)
            .iter()
            .any(|o| output_word(o, 0).contains(U256::ONE))
    );
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 32), &[42]);
        only(&read(&outcome.store, A, 0), &[42]);
    }
}

#[test]
fn static_restriction_is_inherited_by_unknown_delegatecall_and_callcode() {
    for op in [opcode::DELEGATECALL, opcode::CALLCODE] {
        let a = readonly_parent(op);
        let mut world = new_world();
        insert(&mut world, A, &a, &[(0, 42)]);
        let mut input = entry(EvmByteArray::<NumericValue>::empty());
        input.environment.is_static = true;
        let analysis = run(world, input);
        complete_unknown(&analysis);
        assert!(
            returns(&analysis)
                .iter()
                .any(|o| output_word(o, 0).contains(U256::ONE))
        );
        for outcome in returns(&analysis) {
            only(&output_word(outcome, 32), &[42]);
            only(&read(&outcome.store, A, 0), &[42]);
        }
    }
}

/// No concrete slot fact is supplied: equality depends on retaining the same
/// symbolic Store root, rather than reconstructing two unrelated unknowns.
fn check_static_read_identity(op: u8) {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .push(0)
        .op(opcode::SLOAD)
        .call(op, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .op(opcode::EQ)
        .return_top();
    a.label("callback").op(opcode::STOP);
    let code = a.finish();
    let mut world = new_world();
    let mut account = EvmAccount::<NumericValue>::from_hex(
        &alloy_primitives::hex::encode(&code.bytes),
        world.fork(),
    )
    .unwrap();
    account.balance = word(1_000_000);
    account.storage_unknown = true;
    world.insert(address(A), account).unwrap();
    let mut input = entry(EvmByteArray::<NumericValue>::empty());
    input.environment.is_static = op != opcode::STATICCALL;
    let analysis = run(world, input);
    complete_unknown(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[1]);
    }
}

#[test]
fn initially_unknown_slot_retains_old_new_identity_across_unknown_staticcall() {
    check_static_read_identity(opcode::STATICCALL);
}

#[test]
fn initially_unknown_slot_retains_identity_across_inherited_static_delegatecall() {
    check_static_read_identity(opcode::DELEGATECALL);
}

#[test]
fn initially_unknown_slot_retains_identity_across_inherited_static_callcode() {
    check_static_read_identity(opcode::CALLCODE);
}

#[test]
fn unknown_delegatecall_entering_known_c_keeps_c_address_in_normal_call() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .call(opcode::DELEGATECALL, U as u16, 0, (0, 0), (0, 32))
        .op(opcode::ISZERO)
        .jump("failed", true)
        .return_range(0, 32);
    a.label("callback").op(opcode::STOP);
    a.label("failed").push(0).push(0).op(opcode::REVERT);
    let mut c = Assembly::default();
    c.op(opcode::ADDRESS).return_top();
    let mut u = Assembly::default();
    u.call(opcode::CALL, C as u16, 0, (0, 0), (0, 32))
        .op(opcode::ISZERO)
        .jump("failed", true)
        .return_range(0, 32);
    u.label("failed").push(0).push(0).op(opcode::REVERT);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    insert(&mut world, C, &c.finish(), &[]);
    let input = entry(EvmByteArray::<NumericValue>::empty());
    let concrete = oracle::concrete_unknown(&world, U, &u.finish());
    let observed = oracle::execute(&concrete, &input);
    assert_eq!(observed.output_word(0), U256::from(C));
    assert!(observed.steps.iter().any(|step| {
        step.code == address(C) && step.owner == address(C) && step.caller == address(A)
    }));
    let analysis = run(world, input);
    complete_unknown(&analysis);
    observed.covers(&analysis);
    let normal_c: Vec<_> = analysis
        .procedure_summaries()
        .iter()
        .flat_map(|summary| &summary.procedures)
        .filter(|procedure| {
            procedure.frame.code_address == address(C) && procedure.frame.address == address(C)
        })
        .collect();
    assert!(
        !normal_c.is_empty(),
        "normal known C entry provenance is absent"
    );
    for procedure in normal_c {
        let outputs: Vec<_> = procedure
            .outputs
            .iter()
            .filter(|output| output.kind == evm_abstract::analysis::OutcomeKind::Return)
            .collect();
        assert!(!outputs.is_empty());
        for output in outputs {
            only(
                &output
                    .data
                    .read_word(&word(0), native::product().borrowed()),
                &[C],
            );
        }
    }
}

#[test]
fn actual_unknown_code_contexts_match_call_delegatecall_callcode_and_staticcall_in_revm() {
    for op in [
        opcode::CALL,
        opcode::DELEGATECALL,
        opcode::CALLCODE,
        opcode::STATICCALL,
    ] {
        let mut a = Assembly::default();
        a.op(opcode::CALLER)
            .push(USER as u16)
            .op(opcode::EQ)
            .op(opcode::ISZERO)
            .jump("callback", true)
            .store(0, 42)
            .call(op, U as u16, 7, (0, 0), (0, 96))
            .op(opcode::ISZERO)
            .jump("failed", true)
            .push(0)
            .op(opcode::SLOAD)
            .mstore(96)
            .return_range(0, 128);
        a.label("callback").op(opcode::STOP);
        a.label("failed").push(0).push(0).op(opcode::REVERT);
        let mut u = Assembly::default();
        u.op(opcode::ADDRESS)
            .mstore(0)
            .op(opcode::CALLER)
            .mstore(32)
            .op(opcode::CALLVALUE)
            .mstore(64);
        if op != opcode::STATICCALL {
            u.store(0, 99);
        }
        u.return_range(0, 96);
        let mut world = new_world();
        insert(&mut world, A, &a.finish(), &[]);
        let concrete = oracle::concrete_unknown(&world, U, &u.finish());
        let input = entry(EvmByteArray::<NumericValue>::empty());
        let observed = oracle::execute(&concrete, &input);
        let owner = if matches!(op, opcode::DELEGATECALL | opcode::CALLCODE) {
            A
        } else {
            U
        };
        let caller = if op == opcode::DELEGATECALL { USER } else { A };
        let value = if matches!(op, opcode::DELEGATECALL | opcode::STATICCALL) {
            0
        } else {
            7
        };
        assert_eq!(observed.output_word(0), U256::from(owner));
        assert_eq!(observed.output_word(32), U256::from(caller));
        assert_eq!(observed.output_word(64), U256::from(value));
        assert_eq!(
            observed.output_word(96),
            U256::from(if owner == A { 99 } else { 42 })
        );
        assert!(observed.steps.iter().any(|step| step.code == address(U)
            && step.owner == address(owner)
            && step.caller == address(caller)
            && step.value == U256::from(value)
            && step.is_static == (op == opcode::STATICCALL)));
        let analysis = run(world, input);
        complete_unknown(&analysis);
        observed.covers(&analysis);
    }
}
