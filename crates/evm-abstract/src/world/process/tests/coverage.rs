use super::{address, budget, catalogue, domain, prepare, store, word};
use crate::{
    Fork,
    domain::{EvmValue, NumericValue},
    world::{
        Code, EvmAccount, EvmStore, EvmWorld,
        process::{
            CallRoles, CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, ProcessAction,
            ProcessCallOperands, ProcessOwner, ProcessTargetAddress,
        },
    },
};
use alloy_primitives::{Address, U256, keccak256};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter},
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

#[test]
fn outside_storage_is_independent_and_local_effects_preserve_named_state_and_rollback() {
    let domain = domain();
    let catalogue = catalogue();
    let input = store(&domain);
    let residual = prepare(&input, &catalogue, ProcessOwner::Residual, &domain);
    let effects = residual
        .storage_effect(false, domain.borrowed(), &mut budget())
        .unwrap();
    assert!(
        residual
            .equivalent(&effects, domain.borrowed(), &mut budget())
            .unwrap()
    );
    let replay = effects
        .instantiate_for_process(domain.borrowed(), &mut budget())
        .unwrap();
    for (owner, expected) in [(0x100, 11), (0x200, 22)] {
        assert_eq!(
            replay
                .read(address(owner), &word(0), domain.borrowed())
                .singleton(),
            Some(U256::from(expected))
        );
        assert_eq!(
            replay.read_balance(address(owner)),
            input.read_balance(address(owner))
        );
        assert_eq!(replay.code(address(owner)), input.code(address(owner)));
        assert_eq!(
            replay.existence(address(owner)),
            input.existence(address(owner))
        );
    }
    for outside in [0xaaa, 0xbbb, 0xccc] {
        assert!(
            replay
                .read(address(outside), &word(0), domain.borrowed())
                .contains(U256::MAX)
        );
        assert!(
            replay
                .read_transient(address(outside), &word(0), domain.borrowed())
                .contains(U256::MAX)
        );
    }
    let outside_a = replay.read(address(0xaaa), &word(0), domain.borrowed());
    let outside_b = replay.read(address(0xbbb), &word(0), domain.borrowed());
    assert!(!outside_a.same_identity(&outside_b));
    if let (Some(a), Some(b)) = (outside_a.expression(), outside_b.expression()) {
        assert_ne!(
            a, b,
            "per-address storage roots must not equate outside accounts"
        );
    }
    let equality = domain
        .borrowed()
        .apply_budgeted(opcode::EQ, &[outside_a, outside_b], &mut budget())
        .unwrap()
        .value;
    assert!(equality.contains(U256::ZERO) && equality.contains(U256::ONE));
    assert!(
        replay
            .addresses()
            .iter()
            .all(|owner| catalogue.named().contains(owner))
    );

    let named = prepare(
        &input,
        &catalogue,
        ProcessOwner::Named(address(0x100)),
        &domain,
    );
    let static_effect = named
        .storage_effect(true, domain.borrowed(), &mut budget())
        .unwrap();
    assert!(
        named
            .equivalent(&static_effect, domain.borrowed(), &mut budget())
            .unwrap()
    );
    let changed = named
        .storage_effect(false, domain.borrowed(), &mut budget())
        .unwrap();
    let changed = changed
        .instantiate_for_process(domain.borrowed(), &mut budget())
        .unwrap();
    assert!(
        changed
            .read(address(0x100), &word(0), domain.borrowed())
            .contains(U256::MAX)
    );
    assert_eq!(
        changed
            .read(address(0x200), &word(0), domain.borrowed())
            .singleton(),
        Some(U256::from(22))
    );

    for _entered_exit in ["Revert", "Failure"] {
        let saved = input.snapshot();
        let before = input.clone();
        let mut child = named
            .storage_effect(false, domain.borrowed(), &mut budget())
            .unwrap()
            .instantiate_for_process(domain.borrowed(), &mut budget())
            .unwrap();
        child.write(address(0x100), &word(0), &word(99), domain.borrowed());
        child.write_transient(address(0x200), &word(0), &word(88), domain.borrowed());
        child.restore(saved);
        assert_eq!(
            child, before,
            "complete native caller checkpoint must survive process rollback"
        );
    }
}

fn emit_call(action: ProcessAction, target: Address, value: u8, input: u8, output: u8) -> Vec<u8> {
    let mut bytes = vec![
        opcode::PUSH1,
        output,
        opcode::PUSH0,
        opcode::PUSH1,
        input,
        opcode::PUSH0,
    ];
    let op = match action {
        ProcessAction::Call => opcode::CALL,
        ProcessAction::StaticCall => opcode::STATICCALL,
        ProcessAction::DelegateCall => opcode::DELEGATECALL,
        ProcessAction::CallCode => opcode::CALLCODE,
        _ => unreachable!(),
    };
    if matches!(action, ProcessAction::Call | ProcessAction::CallCode) {
        bytes.extend([opcode::PUSH1, value]);
    }
    bytes.push(opcode::PUSH20);
    bytes.extend_from_slice(target.as_slice());
    bytes.extend([opcode::PUSH3, 0x0f, 0x42, 0x40, op]);
    bytes
}

fn insert(db: &mut InMemoryDB, owner: Address, bytes: Vec<u8>, slot: u64) {
    let code = Bytecode::new_raw(Bytes::from(bytes));
    db.insert_account_info(
        owner,
        AccountInfo::new(U256::from(1_000_000), 0, code.hash_slow(), code),
    );
    db.insert_account_storage(owner, U256::ZERO, U256::from(slot))
        .unwrap();
}

#[derive(Default)]
struct Trace(BTreeSet<(Address, Address, Address, U256)>);

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        if let Some(source) = interpreter.input.bytecode_address {
            self.0.insert((
                source,
                interpreter.input.target_address,
                interpreter.input.caller_address,
                interpreter.input.call_value,
            ));
        }
    }
}

#[test]
fn revm_two_outside_owners_known_code_call_delegate_callcode_static_and_rollback_are_covered() {
    let domain = domain();
    let mut witnesses = 0;
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for outside in [0xaaa, 0xbbb] {
            for action in [
                ProcessAction::Call,
                ProcessAction::DelegateCall,
                ProcessAction::CallCode,
            ] {
                for is_static in [false, true] {
                    for terminal in [0, 1, 2] {
                        let mut leaf = vec![
                            opcode::PUSH1,
                            99,
                            opcode::PUSH0,
                            opcode::SSTORE,
                            opcode::PUSH1,
                            88,
                            opcode::PUSH1,
                            1,
                            opcode::TSTORE,
                        ];
                        leaf.extend_from_slice(match terminal {
                            0 => &[opcode::STOP],
                            1 => &[opcode::PUSH0, opcode::PUSH0, opcode::REVERT],
                            _ => &[opcode::INVALID],
                        });
                        let mut driver =
                            emit_call(action, address(0x200), if is_static { 0 } else { 9 }, 0, 0);
                        driver.extend([opcode::POP, opcode::STOP]);
                        let mut root = emit_call(
                            if is_static {
                                ProcessAction::StaticCall
                            } else {
                                ProcessAction::Call
                            },
                            address(outside),
                            if is_static { 0 } else { 7 },
                            0,
                            0,
                        );
                        root.extend([opcode::POP, opcode::STOP]);
                        let mut world = EvmWorld::<NumericValue>::new(fork, "process-owner-revm");
                        for (owner, bytes, slot) in
                            [(0x100, root.clone(), 11), (0x200, leaf.clone(), 22)]
                        {
                            let mut account =
                                EvmAccount::from_hex(&alloy_primitives::hex::encode(bytes), fork)
                                    .unwrap();
                            account.storage.insert(U256::ZERO, word(slot));
                            world.insert(address(owner), account).unwrap();
                        }
                        // The abstract process sees unknown outside code; revm
                        // chooses one concrete implementation for each witness.
                        for (owner, slot) in [(0xaaa, 33), (0xbbb, 44)] {
                            let mut account = EvmAccount::unknown();
                            account.storage.insert(U256::ZERO, word(slot));
                            world.insert(address(owner), account).unwrap();
                        }
                        let catalogue = OwnerCatalogue::freeze(
                            &[address(0x100), address(0x200)],
                            &[
                                CapturedProcessCode {
                                    source: address(0x100),
                                    hash: keccak256(&root),
                                    fork,
                                },
                                CapturedProcessCode {
                                    source: address(0x200),
                                    hash: keccak256(&leaf),
                                    fork,
                                },
                            ],
                            OwnerCatalogueLimits { names: 2, codes: 2 },
                            &mut budget(),
                        )
                        .unwrap();
                        let roles = CallRoles::for_action(action, is_static).unwrap();
                        catalogue
                            .admit_code(
                                CapturedProcessCode {
                                    source: address(0x200),
                                    hash: keccak256(&leaf),
                                    fork,
                                },
                                &mut budget(),
                            )
                            .unwrap();
                        let owner = roles.child_owner(
                            catalogue.classify(address(outside)),
                            ProcessOwner::Named(address(0x200)),
                        );
                        let input = EvmStore::new(&world, domain.borrowed());
                        let checkpoint = prepare(&input, &catalogue, owner, &domain);
                        let effects = checkpoint
                            .storage_effect(roles.is_static, domain.borrowed(), &mut budget())
                            .unwrap();
                        let bound = if terminal == 0 && !is_static {
                            &effects
                        } else {
                            &checkpoint
                        };
                        let bound = bound
                            .instantiate_for_process(domain.borrowed(), &mut budget())
                            .unwrap();
                        let parent = CallRoles::for_action(ProcessAction::Call, false)
                            .unwrap()
                            .bind(
                                ProcessTargetAddress::Independent,
                                &ProcessCallOperands {
                                    address: word(0x100),
                                    caller: word(0x900),
                                    value: word(0),
                                },
                                domain.borrowed(),
                                &mut budget(),
                            )
                            .unwrap();
                        let parent = ProcessCallOperands {
                            address: parent.address,
                            caller: word(0x100),
                            value: word(if is_static { 0 } else { 7 }),
                        };
                        let operands = roles
                            .bind(
                                ProcessTargetAddress::Exact(address(0x200)),
                                &parent,
                                domain.borrowed(),
                                &mut budget(),
                            )
                            .unwrap();

                        let mut db = InMemoryDB::default();
                        insert(&mut db, address(0x100), root, 11);
                        insert(&mut db, address(0x200), leaf.clone(), 22);
                        for (outside_owner, slot) in [(0xaaa, 33), (0xbbb, 44)] {
                            insert(&mut db, address(outside_owner), driver.clone(), slot);
                        }
                        db.insert_account_info(
                            address(0x900),
                            AccountInfo {
                                balance: U256::from(100_000_000),
                                ..AccountInfo::default()
                            },
                        );
                        let context = Context::mainnet()
                            .modify_cfg_chained(|cfg| {
                                cfg.set_spec_and_mainnet_gas_params(fork.spec_id())
                            })
                            .with_db(db);
                        let mut evm = context.build_mainnet_with_inspector(Trace::default());
                        let result = evm
                            .inspect_tx(
                                TxEnv::builder()
                                    .caller(address(0x900))
                                    .kind(TxKind::Call(address(0x100)))
                                    .gas_limit(10_000_000)
                                    .build()
                                    .unwrap(),
                            )
                            .unwrap();
                        assert!(result.result.is_success());
                        let source_frames: Vec<_> = evm
                            .inspector
                            .0
                            .iter()
                            .filter(|(source, _, _, _)| *source == address(0x200))
                            .collect();
                        assert_eq!(
                            source_frames.len(),
                            1,
                            "oracle must actually enter the captured code"
                        );
                        let (_, actual_owner, actual_caller, actual_value) = *source_frames[0];
                        assert_eq!(owner, catalogue.classify(actual_owner));
                        assert!(
                            operands
                                .address
                                .contains(U256::from_be_slice(actual_owner.as_slice()))
                        );
                        assert!(
                            operands
                                .caller
                                .contains(U256::from_be_slice(actual_caller.as_slice()))
                        );
                        assert!(operands.value.contains(actual_value));
                        for (actual_owner, account) in &result.state {
                            if [
                                address(0x100),
                                address(0x200),
                                address(0xaaa),
                                address(0xbbb),
                            ]
                            .contains(actual_owner)
                            {
                                for (slot, value) in &account.storage {
                                    assert!(
                                        bound
                                            .read(
                                                *actual_owner,
                                                &EvmValue::constant(*slot),
                                                domain.borrowed()
                                            )
                                            .contains(value.present_value()),
                                        "uncovered {fork:?} {action:?} static={is_static} terminal={terminal} owner={actual_owner} slot={slot}"
                                    );
                                }
                            }
                        }
                        assert_eq!(
                            bound
                                .read(address(0x100), &word(0), domain.borrowed())
                                .singleton(),
                            Some(U256::from(11))
                        );
                        if action != ProcessAction::Call || terminal != 0 || is_static {
                            assert_eq!(
                                bound
                                    .read(address(0x200), &word(0), domain.borrowed())
                                    .singleton(),
                                Some(U256::from(22))
                            );
                        }
                        let Some(Code::Runtime(program)) = bound.code(address(0x200)) else {
                            panic!("captured code lost")
                        };
                        assert_eq!(program.bytes(), leaf);
                        assert_eq!(program.fork(), fork);
                        witnesses += 1;
                    }
                }
            }
        }
    }
    assert_eq!(witnesses, 108);
    eprintln!(
        "108 concrete revm storage/role/code witnesses covered; process solver hooks remain external"
    );
}

#[test]
fn revm_residual_call_reentry_and_distinct_targets_witness_both_equality_outcomes() {
    let mut seen = BTreeSet::new();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for target in [0xaaa, 0xbbb] {
            // The recursive same-address invocation takes the leaf branch
            // because its input is one byte; the initial invocation is empty.
            let mut driver = vec![opcode::CALLDATASIZE, opcode::PUSH2, 0, 0, opcode::JUMPI];
            driver.extend(emit_call(ProcessAction::Call, address(target), 0, 1, 32));
            driver.extend([
                opcode::POP,
                opcode::PUSH1,
                32,
                opcode::PUSH0,
                opcode::RETURN,
            ]);
            let at = driver.len() as u16;
            driver[2..4].copy_from_slice(&at.to_be_bytes());
            let leaf = [
                opcode::JUMPDEST,
                opcode::ADDRESS,
                opcode::CALLER,
                opcode::EQ,
                opcode::PUSH0,
                opcode::MSTORE,
                opcode::PUSH1,
                32,
                opcode::PUSH0,
                opcode::RETURN,
            ];
            driver.extend(leaf);
            let mut db = InMemoryDB::default();
            insert(&mut db, address(0xaaa), driver, 0);
            insert(&mut db, address(0xbbb), leaf.to_vec(), 0);
            db.insert_account_info(
                address(0x900),
                AccountInfo {
                    balance: U256::from(100_000_000),
                    ..AccountInfo::default()
                },
            );
            let context = Context::mainnet()
                .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
                .with_db(db);
            let mut evm = context.build_mainnet_with_inspector(Trace::default());
            let result = evm
                .inspect_tx(
                    TxEnv::builder()
                        .caller(address(0x900))
                        .kind(TxKind::Call(address(0xaaa)))
                        .gas_limit(10_000_000)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert!(result.result.is_success());
            let equal = U256::from_be_slice(result.result.output().unwrap());
            assert_eq!(equal, U256::from(u8::from(target == 0xaaa)));
            seen.insert(equal);
        }
    }
    assert_eq!(seen, BTreeSet::from([U256::ZERO, U256::ONE]));
}
