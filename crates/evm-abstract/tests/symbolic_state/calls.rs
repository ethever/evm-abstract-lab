use super::{
    oracle,
    support::{
        Assembly, ROOT, USER, address, complete, config, construct_world, entry, no_bad_branch,
        return_words, run_world, word,
    },
};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{MachineEdgeKind, OutcomeKind},
    world::{EvmAccount, EvmByteArray, EvmWorld},
};
use revm_bytecode::opcode;

#[test]
fn all_call_kinds_keep_code_state_caller_value_input_and_private_memory() {
    let mut child = Assembly::default();
    child
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .push(160)
        .op(opcode::MSTORE);
    for (op, offset) in [
        (opcode::ADDRESS, 0),
        (opcode::CALLER, 32),
        (opcode::CALLVALUE, 64),
        (opcode::CALLDATASIZE, 96),
    ] {
        child.op(op).push(offset).op(opcode::MSTORE);
    }
    child
        .load_input(0)
        .push(128)
        .op(opcode::MSTORE)
        .push(192)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let child = child.finish();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for op in [
            opcode::CALL,
            opcode::STATICCALL,
            opcode::CALLCODE,
            opcode::DELEGATECALL,
        ] {
            let mut root = Assembly::default();
            root.push(7)
                .op(opcode::PUSH0)
                .op(opcode::MSTORE)
                .call(op, 0x200, 9, (0, 32), (0x400, 192))
                .op(opcode::ISZERO)
                .jump("outside", true)
                .push(192)
                .push(0x400)
                .op(opcode::RETURN)
                .label("outside")
                .return_word(0);
            let root = root.finish();
            let world = construct_world(fork, &[(ROOT, &root.code), (0x200, &child.code)]);
            let entry = entry(EvmByteArray::<NumericValue>::empty());
            let analysis = run_world(world.clone(), entry.clone(), config());
            complete(&analysis);
            let owner = if matches!(op, opcode::CALLCODE | opcode::DELEGATECALL) {
                ROOT
            } else {
                0x200
            };
            let caller = if op == opcode::DELEGATECALL {
                USER
            } else {
                ROOT
            };
            let value = if matches!(op, opcode::CALL | opcode::CALLCODE) {
                9
            } else {
                0
            };
            let expected: Vec<_> = [owner, caller, value, 32, 7, 0]
                .into_iter()
                .map(U256::from)
                .collect();
            let concrete = oracle::compare(&world, &entry, &analysis);
            let bytes: Vec<_> = expected
                .iter()
                .flat_map(|word| word.to_be_bytes::<32>())
                .collect();
            assert_eq!(concrete.output, bytes, "fork={fork}, opcode={op:#x}");
            let successes: Vec<_> = analysis
                .outcomes()
                .iter()
                .filter(|outcome| {
                    outcome.kind == OutcomeKind::Return
                        && outcome.data.len().singleton() == Some(U256::from(192))
                })
                .collect();
            assert!(!successes.is_empty());
            for outcome in successes {
                for (index, expected) in expected.iter().enumerate() {
                    let actual = outcome
                        .data
                        .read_word(&word(index as u64 * 32), native::product().borrowed());
                    assert_eq!(
                        actual.singleton(),
                        Some(*expected),
                        "fork={fork} opcode={op:#x} word={index} numeric={:?} limited={} nodes={} depth={} diagnostics={:?}",
                        actual.numeric(),
                        actual.symbolic_limit_reached(),
                        outcome.data.state_root().nodes(),
                        outcome.data.state_root().depth(),
                        analysis.diagnostics()
                    );
                }
            }
        }
    }
}

#[test]
fn delegate_inherits_the_parents_nonzero_value_and_original_caller() {
    let mut root = Assembly::default();
    root.call(opcode::DELEGATECALL, 0x200, 0, (0, 0), (0, 32))
        .op(opcode::ISZERO)
        .jump("outside", true)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .return_top()
        .label("outside")
        .return_word(0);
    let world = construct_world(
        Fork::Osaka,
        &[(ROOT, &root.finish().code), (0x200, "345f5260205ff3")],
    );
    let mut entry = entry(EvmByteArray::<NumericValue>::empty());
    entry.environment.value = word(42);
    let analysis = run_world(world, entry, config());
    return_words(&analysis, &[0, 42]);
    let frames: Vec<_> = analysis
        .states()
        .iter()
        .filter(|state| state.key.frames.len() == 2)
        .collect();
    assert!(!frames.is_empty());
    for state in frames {
        assert_eq!(state.active().address, address(ROOT));
        assert_eq!(state.active().caller.as_concrete(), Some(address(USER)));
        assert_eq!(
            state.entry.active().call_value.singleton(),
            Some(U256::from(42))
        );
    }
}

#[test]
fn static_nonzero_callcode_enters_readonly_code_without_a_transfer() {
    static_callcode(false, false);
}

#[test]
fn static_nonzero_callcode_keeps_state_write_restrictions() {
    static_callcode(true, false);
}

#[test]
fn static_callcode_still_checks_the_current_accounts_balance() {
    static_callcode(false, true);
}

fn static_callcode(writes: bool, insufficient: bool) {
    let mut middle = Assembly::default();
    middle
        .call(opcode::CALLCODE, 0x300, 9, (0, 0), (0, 32))
        .push(32)
        .op(opcode::MSTORE)
        .push(64)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let middle = middle.finish();
    let mut root = Assembly::default();
    root.call(opcode::STATICCALL, 0x200, 0, (0, 0), (0, 64))
        .op(opcode::POP)
        .push(64)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let root = root.finish();
    let child = if writes {
        "60075f5500"
    } else {
        "345f5260205ff3"
    };
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = EvmWorld::<NumericValue>::new(fork, "issue111:static-callcode");
        for (owner, code, balance) in [
            (ROOT, root.code.as_str(), 1_000_000),
            (
                0x200,
                middle.code.as_str(),
                if insufficient { 8 } else { 1_000_000 },
            ),
            (0x300, child, 1_000_000),
        ] {
            let mut account = EvmAccount::<NumericValue>::from_hex(code, fork).unwrap();
            account.balance = word(balance);
            world.insert(address(owner), account).unwrap();
        }
        let entry = entry(EvmByteArray::<NumericValue>::empty());
        let analysis = run_world(world.clone(), entry.clone(), config());
        complete(&analysis);
        let concrete = oracle::compare(&world, &entry, &analysis);
        let expected = if writes || insufficient {
            [0_u64, 0]
        } else {
            [9, 1]
        };
        assert_eq!(
            concrete.output,
            expected
                .into_iter()
                .flat_map(|n| U256::from(n).to_be_bytes::<32>())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            concrete.accounts[&address(0x200)].balance,
            U256::from(if insufficient { 8 } else { 1_000_000 })
        );
        assert_eq!(
            concrete.accounts[&address(0x300)].balance,
            U256::from(1_000_000)
        );
        let children: Vec<_> = analysis
            .states()
            .iter()
            .filter(|state| state.active().code_address == address(0x300))
            .collect();
        if insufficient {
            assert!(
                children.is_empty(),
                "CALLCODE value needs caller balance even without transfer"
            );
            assert!(
                concrete
                    .steps
                    .iter()
                    .all(|step| step.code_address != Some(address(0x300)))
            );
        } else {
            assert!(!children.is_empty());
            for state in children {
                assert_eq!(state.active().address, address(0x200));
                assert_eq!(state.active().caller.as_concrete(), Some(address(0x200)));
                assert!(state.active().is_static);
                assert_eq!(
                    state.entry.active().call_value.singleton(),
                    Some(U256::from(9))
                );
            }
            if writes {
                assert!(
                    analysis
                        .edges()
                        .iter()
                        .any(|edge| edge.kind == MachineEdgeKind::Failure
                            && analysis.states()[edge.from].key.frames.len() == 3)
                );
            }
        }
    }
}

fn store_key(code: &mut Assembly, op: u8, value: u16) {
    code.push(value).load_input(0).op(op);
}

fn check_key(code: &mut Assembly, op: u8, value: u16) {
    code.load_input(0).op(op).require_top(value).op(opcode::POP);
}

#[test]
fn nested_delegate_rollback_restores_symbolic_persistent_and_transient_state() {
    let mut child = Assembly::default();
    check_key(&mut child, opcode::SLOAD, 7);
    check_key(&mut child, opcode::TLOAD, 13);
    store_key(&mut child, opcode::SSTORE, 9);
    store_key(&mut child, opcode::TSTORE, 15);
    child.op(opcode::STOP).label("bad").return_word(0xffff);
    let child = child.finish();
    let mut middle = Assembly::default();
    check_key(&mut middle, opcode::SLOAD, 3);
    check_key(&mut middle, opcode::TLOAD, 11);
    store_key(&mut middle, opcode::SSTORE, 7);
    store_key(&mut middle, opcode::TSTORE, 13);
    middle
        .push(32)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATACOPY)
        .call(opcode::DELEGATECALL, 0x300, 0, (0, 32), (0, 0))
        .op(opcode::ISZERO)
        .jump("revert", true);
    check_key(&mut middle, opcode::SLOAD, 9);
    check_key(&mut middle, opcode::TLOAD, 15);
    middle
        .label("revert")
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::REVERT)
        .label("bad")
        .return_word(0xffff);
    let middle = middle.finish();
    let mut root = Assembly::default();
    store_key(&mut root, opcode::SSTORE, 3);
    store_key(&mut root, opcode::TSTORE, 11);
    root.push(32)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATACOPY)
        .call(opcode::DELEGATECALL, 0x200, 0, (0, 32), (0, 0))
        .op(opcode::POP);
    check_key(&mut root, opcode::SLOAD, 3);
    check_key(&mut root, opcode::TLOAD, 11);
    root.return_word(7).label("bad").return_word(0xffff);
    let root = root.finish();
    for use_summaries in [false, true] {
        let mut policy = config();
        policy.use_summaries = use_summaries;
        let analysis = run_world(
            construct_world(
                Fork::Osaka,
                &[
                    (ROOT, &root.code),
                    (0x200, &middle.code),
                    (0x300, &child.code),
                ],
            ),
            entry(EvmByteArray::<NumericValue>::unknown()),
            policy,
        );
        assert!(
            analysis
                .states()
                .iter()
                .any(|state| state.key.frames.len() == 3)
        );
        for (fixture, owner) in [(&root, ROOT), (&middle, 0x200), (&child, 0x300)] {
            no_bad_branch(&analysis, fixture, owner);
        }
        return_words(&analysis, &[7]);
    }
}

#[test]
fn identical_code_in_two_accounts_keeps_storage_owners_separate() {
    let mut root = Assembly::default();
    for (target, value) in [(0x200, 7), (0x300, 9)] {
        root.push(value)
            .op(opcode::PUSH0)
            .op(opcode::MSTORE)
            .call(opcode::CALL, target, 0, (0, 32), (0, 0))
            .op(opcode::POP);
    }
    root.op(opcode::STOP);
    let world = construct_world(
        Fork::Osaka,
        &[
            (ROOT, &root.finish().code),
            (0x200, "5f355f5500"),
            (0x300, "5f355f5500"),
        ],
    );
    let entry = entry(EvmByteArray::<NumericValue>::empty());
    let analysis = run_world(world.clone(), entry.clone(), config());
    complete(&analysis);
    let concrete = oracle::compare(&world, &entry, &analysis);
    for (owner, value) in [(0x200, 7), (0x300, 9)] {
        assert_eq!(
            concrete.accounts[&address(owner)].storage[&U256::ZERO],
            U256::from(value)
        );
    }
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        for (owner, expected) in [(0x200, 7), (0x300, 9)] {
            let value = outcome
                .store
                .read(address(owner), &word(0), native::product().borrowed());
            assert!(
                value.constants().is_some_and(|values| {
                    values
                        .iter()
                        .all(|value| *value == U256::ZERO || *value == U256::from(expected))
                }),
                "same code at {owner:#x} inherited another owner's write: {value:?}"
            );
        }
        assert_eq!(
            outcome
                .store
                .read(address(ROOT), &word(0), native::product().borrowed())
                .singleton(),
            Some(U256::ZERO)
        );
    }
}
