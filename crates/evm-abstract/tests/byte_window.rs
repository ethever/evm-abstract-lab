//! Whole-word and short-prefix regressions through real EVM continuations.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status, WorldAnalysis, analyze_world},
    domain::{AbstractValue, Domain},
    world::{Account, ByteArray, Entry, World},
};
use revm_bytecode::opcode;

const PARENT: u64 = 0x101;
const CHILD: u64 = 0x201;
const CALLER: u64 = 0x900;

fn address(n: u64) -> Address {
    Address::from_word(U256::from(n).into())
}

fn word(n: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn push(code: &mut Vec<u8>, n: U256) {
    code.push(opcode::PUSH32);
    code.extend(n.to_be_bytes::<32>());
}

fn mstore(code: &mut Vec<u8>, at: u64) {
    push(code, U256::from(at));
    code.push(opcode::MSTORE);
}

fn finish(code: &mut Vec<u8>, at: u64, len: u64, op: u8) {
    push(code, U256::from(len));
    push(code, U256::from(at));
    code.push(op);
}

fn call(code: &mut Vec<u8>, at: u64, len: u64) {
    for n in [len, at, 0, 0, 0, CHILD, 1_000_000] {
        push(code, U256::from(n));
    }
    code.push(opcode::CALL);
}

fn account(code: &[u8]) -> Account {
    Account::from_hex(&alloy_primitives::hex::encode(code), Fork::Osaka).unwrap()
}

fn run(world: World) -> WorldAnalysis {
    let analysis = analyze_world(
        world,
        Entry::concrete(
            address(PARENT),
            address(CALLER),
            word(0),
            ByteArray::empty(),
        ),
        ExecutionConfig::default(),
    )
    .unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(
        analysis.frontiers().is_empty(),
        "{:?}",
        analysis.frontiers()
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    analysis
}

fn bounded(value: &AbstractValue) {
    assert!(value.contains(U256::from(3)), "{value:?}");
    assert!(value.contains(U256::from(9)), "{value:?}");
    assert!(!value.contains(U256::from(2)), "{value:?}");
    assert!(!value.contains(U256::from(10)), "{value:?}");
}

#[test]
fn selfbalance_return_slice_preserves_the_original_three_to_nine_bound() {
    let mut code = vec![opcode::SELFBALANCE];
    mstore(&mut code, 64);
    finish(&mut code, 64, 32, opcode::RETURN);
    let mut observed = Account::unknown();
    observed.code = account(&code).code;
    observed.balance = AbstractValue::unsigned_range(U256::from(3), U256::from(9)).unwrap();
    let mut world = World::new(Fork::Osaka, "byte-window:bounded-selfbalance");
    world.insert(address(PARENT), observed).unwrap();
    let analysis = run(world);
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        bounded(
            &outcome
                .data
                .read_word_checked(&word(0), Domain::default())
                .unwrap(),
        );
    }
}

#[test]
fn return_and_revert_continue_through_call_copy_mcopy_mload_and_return() {
    for completion in [opcode::RETURN, opcode::REVERT] {
        let mut parent = Vec::new();
        call(&mut parent, 64, 32);
        parent.push(opcode::POP);
        // MCOPY(0, 64, 32); MSTORE(32, MLOAD(0)); RETURN(32, 32).
        for n in [32_u64, 64, 0] {
            push(&mut parent, U256::from(n));
        }
        parent.push(opcode::MCOPY);
        push(&mut parent, U256::ZERO);
        parent.push(opcode::MLOAD);
        mstore(&mut parent, 32);
        finish(&mut parent, 32, 32, opcode::RETURN);
        let mut child = vec![opcode::SELFBALANCE];
        mstore(&mut child, 0);
        finish(&mut child, 0, 32, completion);
        let mut world = World::new(Fork::Osaka, "byte-window:bounded-call-continuation");
        let mut caller = account(&parent);
        caller.balance = word(1_000_000);
        let mut callee = account(&child);
        callee.balance = AbstractValue::unsigned_range(U256::from(3), U256::from(9)).unwrap();
        world.insert(address(PARENT), caller).unwrap();
        world.insert(address(CHILD), callee).unwrap();
        let analysis = run(world);
        for outcome in analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
        {
            let returned = outcome
                .data
                .read_word_checked(&word(0), Domain::default())
                .unwrap();
            bounded(&returned);
            let balance = outcome.store.read_balance(address(CHILD));
            assert_eq!(returned.numeric(), balance.numeric());
            assert_eq!(returned.provenance(), balance.provenance());
        }
    }
}

fn conditional_jump(code: &mut Vec<u8>) -> usize {
    code.push(opcode::PUSH2);
    let at = code.len();
    code.extend([0, 0, opcode::JUMPI]);
    at
}

fn label(code: &mut Vec<u8>, fixup: usize) {
    let destination = u16::try_from(code.len()).unwrap().to_be_bytes();
    code[fixup..fixup + 2].copy_from_slice(&destination);
    code.push(opcode::JUMPDEST);
}

#[test]
fn unknown_one_byte_continuation_preserves_every_uncopied_suffix_byte() {
    let mut code = vec![opcode::CALLER];
    push(&mut code, U256::from(CALLER));
    code.extend([opcode::EQ, opcode::ISZERO]);
    let callback = conditional_jump(&mut code);
    push(&mut code, U256::from_be_bytes([0xaa; 32]));
    mstore(&mut code, 32);
    push(&mut code, U256::from_be_bytes([0xbb; 32]));
    mstore(&mut code, 64);
    call(&mut code, 32, 16);
    code.extend([opcode::POP, opcode::RETURNDATASIZE, opcode::DUP1]);
    push(&mut code, U256::ONE);
    code.extend([opcode::EQ, opcode::ISZERO]);
    let wrong_length = conditional_jump(&mut code);
    mstore(&mut code, 96);
    finish(&mut code, 32, 96, opcode::RETURN);
    label(&mut code, callback);
    code.push(opcode::STOP);
    label(&mut code, wrong_length);
    finish(&mut code, 0, 0, opcode::REVERT);
    let mut world = World::new(Fork::Osaka, "byte-window:unknown-short-continuation");
    let mut caller = account(&code);
    caller.balance = word(1_000_000);
    world.insert(address(PARENT), caller).unwrap();
    let analysis = run(world);
    assert!(!analysis.procedure_summaries().is_empty());
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        assert_eq!(
            outcome
                .data
                .read_word_checked(&word(64), Domain::default())
                .unwrap()
                .singleton(),
            Some(U256::ONE)
        );
        for at in 1..32 {
            assert_eq!(
                outcome.data.byte_at(at, Domain::default()).singleton(),
                Some(U256::from(0xaa)),
                "suffix byte {at}"
            );
        }
        for at in 32..64 {
            assert_eq!(
                outcome.data.byte_at(at, Domain::default()).singleton(),
                Some(U256::from(0xbb)),
                "neighbor byte {at}"
            );
        }
    }
}
