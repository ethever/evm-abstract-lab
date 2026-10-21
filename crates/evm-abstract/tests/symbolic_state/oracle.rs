//! Concrete execution uses the pinned upstream interpreter, without our transfers.
use super::support::address;
use crate::native;
use alloy_primitives::{Address, U256};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    analysis::{EvmWorldAnalysis, OutcomeKind},
    domain::EvmValue,
    world::{EvmEntry, EvmWorld, Existence},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::{TxEnv, result::ExecutionResult},
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(super) struct Step {
    pub(super) address: Address,
    pub(super) code_address: Option<Address>,
    pub(super) caller: Address,
    pub(super) is_static: bool,
    pub(super) pc: usize,
    pub(super) stack: Vec<U256>,
}

#[derive(Default, Debug)]
struct Trace(Vec<Step>);

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.0.push(Step {
            address: interpreter.input.target_address,
            code_address: interpreter.input.bytecode_address,
            caller: interpreter.input.caller_address,
            is_static: interpreter.runtime_flag.is_static,
            pc: interpreter.bytecode.pc(),
            stack: interpreter.stack.data().clone(),
        });
    }
}

#[derive(Debug)]
pub(super) struct AccountEffect {
    pub(super) nonce: u64,
    pub(super) balance: U256,
    pub(super) storage: BTreeMap<U256, U256>,
    pub(super) code: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(super) struct Concrete {
    pub(super) kind: OutcomeKind,
    pub(super) output: Vec<u8>,
    pub(super) accounts: BTreeMap<Address, AccountEffect>,
    pub(super) steps: Vec<Step>,
}

pub(super) fn compare(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    analysis: &EvmWorldAnalysis<NumericValue>,
) -> Concrete {
    assert!(
        !entry.environment.is_static,
        "revm static cases need a STATICCALL wrapper"
    );
    let mut db = InMemoryDB::default();
    for (owner, account) in world.accounts() {
        if account.existence == Existence::Absent {
            continue;
        }
        assert!(
            !account.storage_unknown,
            "revm requires a complete initial storage snapshot"
        );
        let code = Bytecode::new_raw(Bytes::from(world.raw_account_code(*owner).unwrap()));
        db.insert_account_info(
            *owner,
            AccountInfo::new(
                account.balance.singleton().unwrap(),
                u64::try_from(account.nonce.singleton().unwrap()).unwrap(),
                code.hash_slow(),
                code,
            ),
        );
        for (slot, value) in &account.storage {
            db.insert_account_storage(*owner, *slot, value.singleton().unwrap())
                .unwrap();
        }
    }
    let caller = entry.environment.caller.as_concrete().unwrap();
    assert_ne!(caller, entry.address);
    db.insert_account_info(
        caller,
        AccountInfo {
            balance: U256::from(100_000_000),
            ..AccountInfo::default()
        },
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| {
            cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id());
        })
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace::default());
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(caller)
                .kind(TxKind::Call(entry.address))
                .gas_limit(10_000_000)
                .data(
                    entry
                        .environment
                        .calldata
                        .exact_bytes(native::product().borrowed())
                        .unwrap()
                        .into(),
                )
                .value(entry.environment.value.singleton().unwrap())
                .build()
                .unwrap(),
        )
        .unwrap();
    let kind = if result.result.is_success() {
        OutcomeKind::Return
    } else if matches!(result.result, ExecutionResult::Revert { .. }) {
        OutcomeKind::Revert
    } else {
        OutcomeKind::Failure
    };
    let output = result
        .result
        .output()
        .map_or_else(Vec::new, |data| data.to_vec());
    assert!(
        analysis.outcomes().iter().any(|outcome| {
            outcome.kind == kind
                && outcome.data.len().contains(U256::from(output.len()))
                && output.iter().enumerate().all(|(offset, byte)| {
                    outcome
                        .data
                        .byte_at(offset, native::product().borrowed())
                        .contains(U256::from(*byte))
                })
                && result
                    .state
                    .iter()
                    .filter(|(owner, account)| {
                        world.account(**owner).is_some() || account.is_created()
                    })
                    .all(|(owner, account)| {
                        outcome
                            .store
                            .nonce(*owner)
                            .contains(U256::from(account.info.nonce))
                            && outcome
                                .store
                                .read_balance(*owner)
                                .contains(account.info.balance)
                            && account.storage.iter().all(|(slot, value)| {
                                outcome
                                    .store
                                    .read(
                                        *owner,
                                        &EvmValue::<NumericValue>::constant(*slot),
                                        native::product().borrowed(),
                                    )
                                    .contains(value.present_value())
                            })
                            && account.info.code.as_ref().is_none_or(|code| {
                                outcome.store.raw_account_code(*owner).as_deref()
                                    == Some(code.original_bytes().as_ref())
                            })
                    })
        }),
        "no single abstract outcome covers revm output and all effects: {result:?}"
    );
    assert!(
        !evm.inspector.0.is_empty(),
        "the oracle must execute the fixture bytecode"
    );
    // Also check concrete block-entry stacks, so outcome-only Unknown cannot pass.
    for step in &evm.inspector.0 {
        let matching: Vec<_> = analysis
            .states()
            .iter()
            .filter(|state| {
                state.active().address == step.address
                    && step
                        .code_address
                        .is_none_or(|code| state.active().code_address == code)
                    && state.active().caller.as_concrete() == Some(step.caller)
                    && state.active().is_static == step.is_static
                    && state.executed_pcs.contains(&step.pc)
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "concrete frame/instruction missing: {step:?}"
        );
        if matching
            .iter()
            .any(|state| state.executed_pcs.first() == Some(&step.pc))
        {
            assert!(
                matching.iter().any(|state| {
                    let stack = &state.entry.active().stack;
                    stack.len() == step.stack.len()
                        && stack
                            .iter()
                            .zip(&step.stack)
                            .all(|(value, concrete)| value.contains(*concrete))
                }),
                "concrete block-entry stack missing: {step:?}"
            );
        }
    }
    let accounts = result
        .state
        .iter()
        .filter(|(owner, _)| **owner != caller && **owner != address(0))
        .map(|(owner, account)| {
            (
                *owner,
                AccountEffect {
                    nonce: account.info.nonce,
                    balance: account.info.balance,
                    storage: account
                        .storage
                        .iter()
                        .map(|(slot, value)| (*slot, value.present_value()))
                        .collect(),
                    code: account
                        .info
                        .code
                        .as_ref()
                        .map(|code| code.original_bytes().to_vec()),
                },
            )
        })
        .collect();
    evm_abstract::ssa::build_world(analysis)
        .unwrap()
        .verify(analysis)
        .unwrap();
    Concrete {
        kind,
        output,
        accounts,
        steps: evm.inspector.0,
    }
}
