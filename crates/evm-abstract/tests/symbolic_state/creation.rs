use super::{
    oracle,
    support::{Assembly, ROOT, address, complete, config, entry, run_world, word},
};
use crate::native;
use alloy_primitives::{U256, hex, keccak256};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{FrameCode, OutcomeKind},
    world::{EvmAccount, EvmByteArray, EvmWorld, Existence},
};
use revm_bytecode::opcode;

fn push2(bytes: &mut Vec<u8>, number: usize) {
    bytes.push(opcode::PUSH2);
    bytes.extend(u16::try_from(number).unwrap().to_be_bytes());
}

fn init(runtime: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    let offset = prefix.len() + 13;
    push2(&mut bytes, runtime.len());
    push2(&mut bytes, offset);
    bytes.extend([opcode::PUSH0, opcode::CODECOPY]);
    push2(&mut bytes, runtime.len());
    bytes.extend([opcode::PUSH0, opcode::RETURN]);
    assert_eq!(bytes.len(), offset);
    bytes.extend(runtime);
    bytes
}

fn factory(initcode: &[u8], create2: bool, value: u8, post: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push2(&mut bytes, initcode.len());
    push2(&mut bytes, 0);
    bytes.extend([opcode::PUSH0, opcode::CODECOPY]);
    if create2 {
        bytes.extend([opcode::PUSH1, 5]);
    }
    push2(&mut bytes, initcode.len());
    bytes.extend([
        opcode::PUSH0,
        opcode::PUSH1,
        value,
        if create2 {
            opcode::CREATE2
        } else {
            opcode::CREATE
        },
    ]);
    bytes.extend(post);
    let offset = u16::try_from(bytes.len()).unwrap();
    bytes[4..6].copy_from_slice(&offset.to_be_bytes());
    bytes.extend(initcode);
    bytes
}

fn observed_world(
    fork: Fork,
    root: &[u8],
    code_target: &[u8],
    absent: &[alloy_primitives::Address],
) -> EvmWorld<NumericValue> {
    let mut world = EvmWorld::<NumericValue>::new(fork, "issue111:creation-context-and-savepoints");
    for (owner, bytes, nonce, balance) in [(ROOT, root, 5, 1000), (0x200, code_target, 37, 2000)] {
        let mut account = EvmAccount::<NumericValue>::from_hex(&hex::encode(bytes), fork).unwrap();
        account.nonce = word(nonce);
        account.balance = word(balance);
        world.insert(address(owner), account).unwrap();
    }
    for owner in absent {
        world
            .insert(*owner, EvmAccount::<NumericValue>::absent())
            .unwrap();
    }
    world
        .insert(
            alloy_primitives::Address::ZERO,
            EvmAccount::<NumericValue>::absent(),
        )
        .unwrap();
    world
}

#[test]
fn delegate_create_uses_state_owner_nonce_funding_and_runtime_visibility() {
    delegated_creation(opcode::DELEGATECALL, false);
}

#[test]
fn delegate_create2_uses_state_owner_salt_and_updates_creator_nonce() {
    delegated_creation(opcode::DELEGATECALL, true);
}

#[test]
fn callcode_create_uses_state_owner_instead_of_the_code_account() {
    delegated_creation(opcode::CALLCODE, false);
}

fn delegated_creation(call_op: u8, create2: bool) {
    let runtime = hex::decode("602a5f5260205ff3").unwrap();
    // New code is empty while initcode runs. ADDRESS/CALLER/value/calldata/memory
    // observations are stored by that actual creation frame, then read below.
    let prefix = hex::decode("303b5f553360015530600255346003553660045559600555").unwrap();
    let initcode = init(&runtime, &prefix);
    let mut post = Assembly::default();
    post.op(opcode::DUP1)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .op(opcode::DUP1)
        .op(opcode::EXTCODESIZE)
        .push(32)
        .op(opcode::MSTORE)
        .op(opcode::DUP1)
        .op(opcode::EXTCODEHASH)
        .push(64)
        .op(opcode::MSTORE)
        .op(opcode::RETURNDATASIZE)
        .push(96)
        .op(opcode::MSTORE)
        .push(32)
        .push(160)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .push_word(U256::from(1_000_000))
        .op(opcode::CALL)
        .push(128)
        .op(opcode::MSTORE)
        .push(runtime.len() as u16)
        .op(opcode::PUSH0)
        .push(192)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .op(opcode::EXTCODECOPY)
        .op(opcode::POP)
        .push(224)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let target = factory(
        &initcode,
        create2,
        7,
        &hex::decode(post.finish().code).unwrap(),
    );
    let mut root = Assembly::default();
    root.call(call_op, 0x200, 9, (0, 0), (0, 224))
        .op(opcode::ISZERO)
        .jump("outside", true)
        .push(224)
        .op(opcode::PUSH0)
        .op(opcode::RETURN)
        .label("outside")
        .return_word(0);
    let root = hex::decode(root.finish().code).unwrap();
    let destination = if create2 {
        address(ROOT).create2_from_code(U256::from(5).to_be_bytes::<32>(), &initcode)
    } else {
        address(ROOT).create(5)
    };
    let wrong_owner = if create2 {
        address(0x200).create2_from_code(U256::from(5).to_be_bytes::<32>(), &initcode)
    } else {
        address(0x200).create(37)
    };
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let world = observed_world(fork, &root, &target, &[destination, wrong_owner]);
        let entry = entry(EvmByteArray::<NumericValue>::empty());
        let analysis = run_world(world.clone(), entry.clone(), config());
        complete(&analysis);
        let concrete = oracle::compare(&world, &entry, &analysis);
        assert_eq!(concrete.kind, OutcomeKind::Return);
        assert_eq!(concrete.output.len(), 224);
        let returned_words: Vec<_> = concrete
            .output
            .as_chunks::<32>()
            .0
            .iter()
            .map(|bytes| U256::from_be_bytes(*bytes))
            .collect();
        assert_eq!(
            returned_words[..6],
            [
                U256::from_be_slice(destination.as_slice()),
                U256::from(runtime.len()),
                U256::from_be_slice(keccak256(&runtime).as_slice()),
                U256::ZERO,
                U256::from(1),
                U256::from(42)
            ]
        );
        assert_eq!(
            &concrete.output[192..192 + runtime.len()],
            runtime.as_slice()
        );
        assert_eq!(concrete.accounts[&address(ROOT)].nonce, 6);
        assert_eq!(concrete.accounts[&address(ROOT)].balance, U256::from(993));
        assert_eq!(concrete.accounts[&address(0x200)].nonce, 37);
        assert_eq!(concrete.accounts[&address(0x200)].balance, U256::from(2000));
        assert_eq!(
            concrete.accounts[&destination].code.as_deref(),
            Some(runtime.as_slice())
        );
        assert!(analysis.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome.store.raw_account_code(destination).as_deref()
                    == Some(runtime.as_slice())
                && outcome.store.existence(wrong_owner) == Existence::Absent
                && outcome.store.nonce(address(ROOT)).singleton() == Some(U256::from(6))
                && outcome.store.nonce(address(0x200)).singleton() == Some(U256::from(37))
                && [
                    (0, U256::ZERO),
                    (1, U256::from(ROOT)),
                    (2, U256::from_be_slice(destination.as_slice())),
                    (3, U256::from(7)),
                    (4, U256::ZERO),
                    (5, U256::ZERO),
                ]
                .into_iter()
                .all(|(slot, expected)| {
                    outcome
                        .store
                        .read(destination, &word(slot), native::product().borrowed())
                        .singleton()
                        == Some(expected)
                })
        }));
        assert!(
            concrete
                .steps
                .iter()
                .any(|step| step.address == destination && step.code_address.is_none())
        );
        assert!(
            concrete
                .steps
                .iter()
                .any(|step| step.code_address == Some(destination)),
            "immediate CALL must execute deployed runtime"
        );
        evm_abstract::ssa::build_world(&analysis)
            .unwrap()
            .verify(&analysis)
            .unwrap();
    }
}

#[test]
fn failed_init_nonce_belongs_to_its_outer_savepoint_and_preserves_prior_creation() {
    let successful_init = init(&[opcode::STOP], &[]);
    let failed_init = hex::decode("60075f5560425f5360015ffd").unwrap();
    let prior = address(ROOT).create(5);
    let failed = address(ROOT).create(6);
    for outer_revert in [false, true] {
        let mut middle_post = Assembly::default();
        middle_post
            .op(opcode::PUSH0)
            .op(opcode::MSTORE)
            .op(opcode::RETURNDATASIZE)
            .push(32)
            .op(opcode::MSTORE)
            .op(opcode::RETURNDATASIZE)
            .op(opcode::PUSH0)
            .push(64)
            .op(opcode::RETURNDATACOPY)
            .push(96)
            .op(opcode::PUSH0)
            .op(if outer_revert {
                opcode::REVERT
            } else {
                opcode::RETURN
            });
        let middle = factory(
            &failed_init,
            false,
            7,
            &hex::decode(middle_post.finish().code).unwrap(),
        );
        let mut root_post = Assembly::default();
        root_post
            .call(opcode::DELEGATECALL, 0x200, 0, (0, 0), (0x100, 96))
            .op(opcode::POP)
            .op(opcode::DUP1)
            .op(opcode::PUSH0)
            .op(opcode::MSTORE)
            .op(opcode::EXTCODESIZE)
            .push(32)
            .op(opcode::MSTORE)
            .op(opcode::RETURNDATASIZE)
            .push(64)
            .op(opcode::MSTORE)
            .push(96)
            .push(0x100)
            .push(96)
            .op(opcode::MCOPY)
            .push(192)
            .op(opcode::PUSH0)
            .op(opcode::RETURN);
        let root = factory(
            &successful_init,
            false,
            0,
            &hex::decode(root_post.finish().code).unwrap(),
        );
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            let world = observed_world(fork, &root, &middle, &[prior, failed]);
            let entry = entry(EvmByteArray::<NumericValue>::empty());
            let analysis = run_world(world.clone(), entry.clone(), config());
            complete(&analysis);
            let concrete = oracle::compare(&world, &entry, &analysis);
            let expected_nonce = if outer_revert { 6 } else { 7 };
            assert_eq!(concrete.accounts[&address(ROOT)].nonce, expected_nonce);
            assert_eq!(concrete.accounts[&address(ROOT)].balance, U256::from(1000));
            assert_eq!(
                concrete.accounts[&prior].code.as_deref(),
                Some(&[opcode::STOP][..])
            );
            assert_eq!(U256::from_be_slice(&concrete.output[32..64]), U256::from(1));
            assert_eq!(
                U256::from_be_slice(&concrete.output[64..96]),
                U256::from(96)
            );
            assert_eq!(U256::from_be_slice(&concrete.output[96..128]), U256::ZERO);
            assert_eq!(
                U256::from_be_slice(&concrete.output[128..160]),
                U256::from(1)
            );
            assert_eq!(concrete.output[160], 0x42);
            let init_states: Vec<_> = analysis
                .states()
                .iter()
                .filter(|state| {
                    state.active().mode == FrameCode::InitCode && state.active().address == failed
                })
                .collect();
            assert!(!init_states.is_empty());
            for state in init_states {
                assert_eq!(
                    state
                        .entry
                        .active()
                        .saved_store
                        .state()
                        .nonce(address(ROOT))
                        .singleton(),
                    Some(U256::from(7)),
                    "initcode failure rolls back after the creator's nonce increment"
                );
                let delegate = state
                    .entry
                    .call_stack
                    .iter()
                    .find(|frame| frame.key.code_address == address(0x200))
                    .unwrap();
                assert_eq!(
                    delegate
                        .saved_store
                        .state()
                        .nonce(address(ROOT))
                        .singleton(),
                    Some(U256::from(6)),
                    "outer delegate rollback restores the nonce after the parent's earlier CREATE"
                );
            }
            // A pre-entry DELEGATECALL failure leaves nonce=6; entering the
            // factory advances it to 7. Both are valid under abstract gas.
            // The exact savepoints above distinguish the rollback layers.
            let allowed_nonces = if outer_revert { &[6][..] } else { &[6, 7][..] };
            for outcome in analysis
                .outcomes()
                .iter()
                .filter(|outcome| outcome.kind == OutcomeKind::Return)
            {
                assert!(
                    outcome
                        .store
                        .nonce(address(ROOT))
                        .constants()
                        .is_some_and(|values| {
                            values.iter().all(|value| {
                                allowed_nonces
                                    .iter()
                                    .any(|nonce| *value == U256::from(*nonce))
                            })
                        }),
                    "nonce escaped its valid pre-entry/entered paths"
                );
                assert_eq!(outcome.store.existence(failed), Existence::Absent);
                assert_eq!(outcome.store.raw_account_code(failed), Some(Vec::new()));
                assert_eq!(
                    outcome
                        .store
                        .read(failed, &word(0), native::product().borrowed())
                        .singleton(),
                    Some(U256::ZERO)
                );
                assert_eq!(
                    outcome.store.read_balance(address(ROOT)).singleton(),
                    Some(U256::from(1000))
                );
            }
            assert!(
                analysis
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Return
                        && outcome
                            .store
                            .nonce(address(ROOT))
                            .contains(U256::from(expected_nonce))
                        && outcome.store.raw_account_code(prior) == Some(vec![opcode::STOP])
                        && outcome.store.existence(failed) == Existence::Absent
                        && outcome
                            .store
                            .read(failed, &word(0), native::product().borrowed())
                            .singleton()
                            == Some(U256::ZERO)),
                "prior creation and the entered factory's nonce must coexist after rollback"
            );
            if outer_revert {
                assert!(
                    analysis
                        .outcomes()
                        .iter()
                        .filter(|outcome| outcome.kind == OutcomeKind::Return)
                        .all(|outcome| outcome.store.nonce(address(ROOT)).singleton()
                            == Some(U256::from(6)))
                );
            }
        }
    }
}

#[test]
fn reverting_delegate_removes_its_deployed_code_storage_and_funding() {
    let runtime = hex::decode("602a5f5260205ff3").unwrap();
    let initcode = init(&runtime, &hex::decode("60075f55").unwrap());
    let destination = address(ROOT).create(5);
    let mut middle_post = Assembly::default();
    middle_post
        .op(opcode::DUP1)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .op(opcode::EXTCODESIZE)
        .push(32)
        .op(opcode::MSTORE)
        .push(64)
        .op(opcode::PUSH0)
        .op(opcode::REVERT);
    let middle = factory(
        &initcode,
        false,
        7,
        &hex::decode(middle_post.finish().code).unwrap(),
    );
    let mut root = Assembly::default();
    root.push(3)
        .push(9)
        .op(opcode::SSTORE)
        .call(opcode::DELEGATECALL, 0x200, 0, (0, 0), (0, 64))
        .push(64)
        .op(opcode::MSTORE)
        .op(opcode::RETURNDATASIZE)
        .push(96)
        .op(opcode::MSTORE)
        .push_word(U256::from_be_slice(destination.as_slice()))
        .op(opcode::EXTCODESIZE)
        .push(128)
        .op(opcode::MSTORE)
        .push_word(U256::from_be_slice(destination.as_slice()))
        .op(opcode::EXTCODEHASH)
        .push(160)
        .op(opcode::MSTORE)
        .op(opcode::SELFBALANCE)
        .push(192)
        .op(opcode::MSTORE)
        .push(32)
        .push(256)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .push_word(U256::from_be_slice(destination.as_slice()))
        .push_word(U256::from(1_000_000))
        .op(opcode::CALL)
        .push(224)
        .op(opcode::MSTORE)
        .op(opcode::RETURNDATASIZE)
        .push(256)
        .op(opcode::MSTORE)
        .push(288)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let root = hex::decode(root.finish().code).unwrap();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let world = observed_world(fork, &root, &middle, &[destination]);
        let entry = entry(EvmByteArray::<NumericValue>::empty());
        let analysis = run_world(world.clone(), entry.clone(), config());
        complete(&analysis);
        let concrete = oracle::compare(&world, &entry, &analysis);
        let expected = [
            U256::from_be_slice(destination.as_slice()),
            U256::from(runtime.len()),
            U256::ZERO,
            U256::from(64),
            U256::ZERO,
            U256::ZERO,
            U256::from(1000),
            U256::from(1),
            U256::ZERO,
        ];
        assert_eq!(
            concrete.output,
            expected
                .into_iter()
                .flat_map(|value| value.to_be_bytes::<32>())
                .collect::<Vec<_>>()
        );
        assert_eq!(concrete.accounts[&address(ROOT)].nonce, 5);
        assert!(
            concrete
                .steps
                .iter()
                .any(|step| step.address == destination && step.code_address.is_none())
        );
        assert!(
            concrete
                .steps
                .iter()
                .all(|step| step.code_address != Some(destination)),
            "CALL after rollback must not execute the reverted runtime"
        );
        for outcome in analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
        {
            assert_eq!(
                outcome.store.nonce(address(ROOT)).singleton(),
                Some(U256::from(5))
            );
            assert_eq!(
                outcome.store.read_balance(address(ROOT)).singleton(),
                Some(U256::from(1000))
            );
            assert_eq!(outcome.store.existence(destination), Existence::Absent);
            assert_eq!(
                outcome.store.raw_account_code(destination),
                Some(Vec::new())
            );
            assert_eq!(
                outcome
                    .store
                    .read(destination, &word(0), native::product().borrowed())
                    .singleton(),
                Some(U256::ZERO)
            );
            assert_eq!(
                outcome
                    .store
                    .read(address(ROOT), &word(9), native::product().borrowed())
                    .singleton(),
                Some(U256::from(3))
            );
        }
    }
}
