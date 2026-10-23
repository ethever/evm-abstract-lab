//! EIP-214's nonzero static CALLCODE exception, observed through read-only code.
//!
//! CALLCODE's self-transfer still checks the owner's balance. An Any operand
//! bound is not a funding proof. Both admission and refusal are revm witnesses.

use super::{address, budget, domain, emit_call, insert, prepare, word};
use crate::{
    Fork,
    world::{
        EvmAccount, EvmStore, EvmWorld,
        process::{
            CallRoles, CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, ProcessAction,
            ProcessCallOperands, ProcessOwner, ProcessTargetAddress,
        },
    },
};
use alloy_primitives::{Address, U256, hex, keccak256};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{
        CallInputs, CallOutcome, InstructionResult, Interpreter, interpreter::EthInterpreter,
        interpreter_types::RuntimeFlag,
    },
    primitives::TxKind,
    state::AccountInfo,
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

#[derive(Default)]
struct Trace {
    frames: BTreeSet<(Address, Address, Address, U256, bool)>,
    completions: Vec<(Address, InstructionResult)>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        if let Some(source) = interpreter.input.bytecode_address {
            self.frames.insert((
                source,
                interpreter.input.target_address,
                interpreter.input.caller_address,
                interpreter.input.call_value,
                interpreter.runtime_flag.is_static(),
            ));
        }
    }

    fn call_end(&mut self, _: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.completions
            .push((inputs.bytecode_address, outcome.result.result));
    }
}

#[test]
fn funded_static_callcode_value_nine_and_subsequent_delegate_are_covered() {
    let domain = domain();
    // D returns its actual inherited CALLVALUE. C returns its own value and
    // then D's, so both transitions are observable independently of storage.
    let leaf = vec![
        opcode::CALLVALUE,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ];
    let mut captured = vec![
        opcode::CALLVALUE,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH20,
    ];
    captured.extend_from_slice(address(0x300).as_slice());
    captured.extend([
        opcode::PUSH3,
        0x0f,
        0x42,
        0x40,
        opcode::DELEGATECALL,
        opcode::POP,
        opcode::PUSH1,
        64,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    let mut driver = emit_call(ProcessAction::CallCode, address(0x200), 9, 0, 64);
    driver.extend([
        opcode::PUSH1,
        64,
        opcode::MSTORE,
        opcode::PUSH1,
        96,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    // The same root bytecode/epoch covers B=aaa and B=bbb via calldata.
    let root = vec![
        opcode::PUSH1,
        96,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::PUSH0,
        opcode::CALLDATALOAD,
        opcode::PUSH3,
        0x0f,
        0x42,
        0x40,
        opcode::STATICCALL,
        opcode::PUSH1,
        96,
        opcode::MSTORE,
        opcode::PUSH1,
        128,
        opcode::PUSH0,
        opcode::RETURN,
    ];
    let mut cases = 0;
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = EvmWorld::new(fork, "nonzero-static-callcode");
        let codes = [(0x101, &root), (0x200, &captured), (0x300, &leaf)];
        for (owner, code) in codes {
            world
                .insert(
                    address(owner),
                    EvmAccount::from_hex(&hex::encode(code), fork).unwrap(),
                )
                .unwrap();
        }
        let catalogue = OwnerCatalogue::freeze(
            &[address(0x101), address(0x200), address(0x300)],
            &codes.map(|(source, code)| CapturedProcessCode {
                source: address(source),
                hash: keccak256(code),
                fork,
            }),
            OwnerCatalogueLimits { names: 3, codes: 3 },
            &mut budget(),
        )
        .unwrap();
        let input = EvmStore::new(&world, domain.borrowed());
        let prepared = prepare(&input, &catalogue, ProcessOwner::Residual, &domain);
        let unchanged = prepared
            .storage_effect(true, domain.borrowed(), &mut budget())
            .unwrap();
        assert!(
            prepared
                .equivalent(&unchanged, domain.borrowed(), &mut budget())
                .unwrap()
        );
        let caller = ProcessCallOperands {
            address: word(0x101),
            caller: word(0x900),
            value: word(0),
        };
        let parent = CallRoles::for_action(ProcessAction::StaticCall, false)
            .unwrap()
            .bind(
                ProcessTargetAddress::Independent,
                &caller,
                domain.borrowed(),
                &mut budget(),
            )
            .unwrap();
        let callcode_roles = CallRoles::for_action(ProcessAction::CallCode, true).unwrap();
        assert!(callcode_roles.is_static);
        let callcode = callcode_roles
            .bind(
                ProcessTargetAddress::Exact(address(0x200)),
                &parent,
                domain.borrowed(),
                &mut budget(),
            )
            .unwrap();
        let delegate_roles = CallRoles::for_action(ProcessAction::DelegateCall, true).unwrap();
        let delegate = delegate_roles
            .bind(
                ProcessTargetAddress::Exact(address(0x300)),
                &callcode,
                domain.borrowed(),
                &mut budget(),
            )
            .unwrap();
        assert!(delegate_roles.is_static);
        assert_eq!(
            callcode_roles.child_owner(ProcessOwner::Residual, ProcessOwner::Named(address(0x200))),
            ProcessOwner::Residual
        );
        assert_eq!(
            delegate_roles.child_owner(ProcessOwner::Residual, ProcessOwner::Named(address(0x300))),
            ProcessOwner::Residual
        );
        for outside in [0xaaa, 0xbbb] {
            for balance in [8, 9] {
                let mut db = InMemoryDB::default();
                for (owner, code) in codes {
                    insert(&mut db, address(owner), code.clone(), 0);
                }
                insert(&mut db, address(outside), driver.clone(), 0);
                db.accounts.get_mut(&address(outside)).unwrap().info.balance = U256::from(balance);
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
                            .kind(TxKind::Call(address(0x101)))
                            .data(U256::from(outside).to_be_bytes::<32>().to_vec().into())
                            .gas_limit(10_000_000)
                            .build()
                            .unwrap(),
                    )
                    .unwrap();
                assert!(result.result.is_success());
                let output = result.result.output().unwrap();
                assert_eq!(output.len(), 128);
                let words: Vec<_> = output.chunks_exact(32).map(U256::from_be_slice).collect();
                let funded = balance >= 9;
                assert_eq!(
                    words,
                    if funded {
                        vec![U256::from(9), U256::from(9), U256::ONE, U256::ONE]
                    } else {
                        vec![U256::ZERO, U256::ZERO, U256::ZERO, U256::ONE]
                    }
                );
                for (source, bound) in [(0x200, &callcode), (0x300, &delegate)] {
                    let frames: Vec<_> = evm
                        .inspector
                        .frames
                        .iter()
                        .filter(|(code, _, _, _, _)| *code == address(source))
                        .collect();
                    assert_eq!(frames.len(), usize::from(funded));
                    if funded {
                        assert_eq!(
                            *frames[0],
                            (
                                address(source),
                                address(outside),
                                address(outside),
                                U256::from(9),
                                true
                            )
                        );
                        assert!(bound.address.contains(U256::from(outside)));
                        assert!(bound.caller.contains(U256::from(outside)));
                        assert!(bound.value.contains(words[usize::from(source == 0x300)]));
                    }
                }
                let outcomes: Vec<_> = evm
                    .inspector
                    .completions
                    .iter()
                    .filter(|(source, _)| *source == address(0x200))
                    .collect();
                assert_eq!(outcomes.len(), 1);
                if funded {
                    assert!(outcomes[0].1.is_ok());
                    let concrete_parent = ProcessCallOperands {
                        address: word(outside),
                        caller: word(outside),
                        value: word(9),
                    };
                    let exact_delegate = delegate_roles
                        .bind(
                            ProcessTargetAddress::Exact(address(0x300)),
                            &concrete_parent,
                            domain.borrowed(),
                            &mut budget(),
                        )
                        .unwrap();
                    assert_eq!(exact_delegate.value.singleton(), Some(U256::from(9)));
                } else {
                    assert_eq!(outcomes[0].1, InstructionResult::OutOfFunds);
                }
                eprintln!(
                    "PROCESS_OWNER_STATIC_CALLCODE {}",
                    serde_json::json!({
                        "fork":fork,"outside_owner":address(outside),"balance":balance,"callcode_value":9,
                        "funded":funded,"is_static":true,"code_source":address(0x200),"delegate_source":address(0x300),
                        "return_words":words,"catalogue_epoch":catalogue.epoch(),"helper_proves_funding":false,
                    })
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 12);
}
