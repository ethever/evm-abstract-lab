//! Concrete bytecode and journaling belong to revm. Compare an entire witness
//! with one abstract terminal, rather than combining independent alternatives.
use super::support::{address, word};
use crate::native;
use alloy_primitives::{Address, B256, Log, U256};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    analysis::{EvmWorldAnalysis, OutcomeKind},
    world::{EvmEntry, EvmWorld, Existence},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    context_interface::block::BlobExcessGasAndPrice,
    database::InMemoryDB,
    interpreter::{
        CallInputs, CallOutcome, InstructionResult, Interpreter, interpreter::EthInterpreter,
        interpreter_types::Jumps,
    },
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(super) struct Step {
    pub(super) code: Address,
    pub(super) owner: Address,
    pub(super) caller: Address,
    pub(super) is_static: bool,
    pub(super) pc: usize,
    pub(super) opcode: u8,
    pub(super) depth: usize,
    pub(super) storage_write: Option<(U256, U256)>,
}

#[derive(Default)]
struct Trace {
    steps: Vec<Step>,
    depth: usize,
    depth_failures: usize,
    emitted_logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn call(&mut self, _: &mut CTX, _: &mut CallInputs) -> Option<CallOutcome> {
        self.depth += 1;
        None
    }

    fn call_end(&mut self, _: &mut CTX, _: &CallInputs, outcome: &mut CallOutcome) {
        if outcome.result.result == InstructionResult::CallTooDeep {
            self.depth_failures += 1;
        }
        self.depth -= 1;
    }

    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.steps.push(Step {
            code: interpreter
                .input
                .bytecode_address
                .unwrap_or(interpreter.input.target_address),
            owner: interpreter.input.target_address,
            caller: interpreter.input.caller_address,
            is_static: interpreter.runtime_flag.is_static,
            pc: interpreter.bytecode.pc(),
            opcode: interpreter.bytecode.opcode(),
            depth: self.depth,
            storage_write: (interpreter.bytecode.opcode() == revm_bytecode::opcode::SSTORE).then(
                || {
                    (
                        interpreter.stack.peek(0).unwrap(),
                        interpreter.stack.peek(1).unwrap(),
                    )
                },
            ),
        });
    }

    fn log(&mut self, _: &mut CTX, log: Log) {
        self.emitted_logs.push((
            log.address,
            log.data
                .topics()
                .iter()
                .map(|topic| U256::from_be_slice(topic.as_slice()))
                .collect(),
            log.data.data.to_vec(),
        ));
    }
}

pub(super) struct Observation {
    pub(super) kind: OutcomeKind,
    pub(super) data: Vec<u8>,
    pub(super) storage: BTreeMap<(Address, U256), U256>,
    pub(super) balances: BTreeMap<Address, U256>,
    pub(super) logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
    pub(super) steps: Vec<Step>,
    pub(super) depth_failures: usize,
    pub(super) emitted_logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
}

impl Observation {
    pub(super) fn output_word(&self, offset: usize) -> U256 {
        U256::from_be_slice(&self.data[offset..offset + 32])
    }

    pub(super) fn slot(&self, owner: u64, slot: u64) -> U256 {
        self.storage[&(address(owner), U256::from(slot))]
    }

    pub(super) fn covers(&self, analysis: &EvmWorldAnalysis<NumericValue>) {
        assert!(
            analysis.outcomes().iter().any(|outcome| {
                outcome.kind == self.kind
                    && outcome.data.len().contains(U256::from(self.data.len()))
                    && self.data.iter().enumerate().all(|(offset, byte)| {
                        outcome
                            .data
                            .byte_at(offset, native::product().borrowed())
                            .contains(U256::from(*byte))
                    })
                    && self.storage.iter().all(|((owner, slot), value)| {
                        outcome
                            .store
                            .read(
                                *owner,
                                &evm_abstract::domain::EvmValue::<NumericValue>::constant(*slot),
                                native::product().borrowed(),
                            )
                            .contains(*value)
                    })
                    && self.balances.iter().all(|(owner, balance)| {
                        outcome.store.read_balance(*owner).contains(*balance)
                    })
                    && self.logs.iter().all(|(owner, topics, data)| {
                        outcome.store.logs_unknown()
                            || outcome.store.possible_logs().iter().any(|(site, log)| {
                                site.address == *owner
                                    && log.topics.len() == topics.len()
                                    && log
                                        .topics
                                        .iter()
                                        .zip(topics)
                                        .all(|(possible, value)| possible.contains(*value))
                                    && log.data.len().contains(U256::from(data.len()))
                                    && data.iter().enumerate().all(|(offset, byte)| {
                                        log.data
                                            .byte_at(offset, native::product().borrowed())
                                            .contains(U256::from(*byte))
                                    })
                            })
                    })
            }),
            "no common abstract outcome covers revm kind={:?}, data={}, storage={:?}, balances={:?}",
            self.kind,
            alloy_primitives::hex::encode(&self.data),
            self.storage,
            self.balances
        );
    }
}

pub(super) fn execute(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
) -> Observation {
    execute_gas(world, entry, 10_000_000)
}

pub(super) fn execute_gas(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    gas_limit: u64,
) -> Observation {
    let mut db = InMemoryDB::default();
    let mut initial_storage = BTreeMap::new();
    for (owner, account) in world.accounts() {
        assert_ne!(account.existence, Existence::Absent);
        let code = Bytecode::new_raw(Bytes::from(
            world
                .raw_account_code(*owner)
                .expect("concrete captured code"),
        ));
        db.insert_account_info(
            *owner,
            AccountInfo::new(
                account.balance.singleton().expect("concrete balance"),
                u64::try_from(account.nonce.singleton().expect("concrete nonce")).unwrap(),
                code.hash_slow(),
                code,
            ),
        );
        // For partially observed worlds this chooses zero for unobserved slots;
        // the abstract input keeps them unknown. It is one concrete witness.
        for (slot, value) in &account.storage {
            let value = value.singleton().expect("concrete observed slot");
            db.insert_account_storage(*owner, *slot, value).unwrap();
            initial_storage.insert((*owner, *slot), value);
        }
    }
    let caller = entry.environment.caller.as_concrete().unwrap();
    // Caller is an unobserved account in these fixtures; choose sufficient
    // funds for the concrete gas price while comparing every captured balance.
    assert!(world.account(caller).is_none());
    db.insert_account_info(
        caller,
        AccountInfo {
            balance: U256::MAX,
            ..AccountInfo::default()
        },
    );
    db.cache.block_hashes.extend(
        entry
            .environment
            .block_hashes
            .iter()
            .map(|(number, hash)| (*number, *hash)),
    );
    let environment = &entry.environment;
    let basefee = environment
        .base_fee
        .singleton()
        .map_or(0, |value| u64::try_from(value).unwrap());
    let gas_price = environment
        .gas_price
        .singleton()
        .map_or(u128::from(basefee), |value| u128::try_from(value).unwrap());
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| {
            cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id());
            if let Some(value) = environment
                .chain_id
                .as_ref()
                .and_then(|value| value.singleton())
            {
                cfg.chain_id = u64::try_from(value).unwrap();
            }
        })
        .modify_block_chained(|block| {
            block.gas_limit = environment
                .gas_limit
                .singleton()
                .map_or(gas_limit, |value| u64::try_from(value).unwrap());
            block.basefee = basefee;
            if let Some(value) = environment.number.singleton() {
                block.number = value;
            }
            if let Some(value) = environment.timestamp.singleton() {
                block.timestamp = value;
            }
            if let Some(value) = environment.coinbase.as_concrete() {
                block.beneficiary = value;
            }
            if let Some(value) = environment.prevrandao.singleton() {
                block.prevrandao = Some(B256::from(value.to_be_bytes::<32>()));
            }
            if let Some(value) = environment.blob_base_fee.singleton() {
                block.blob_excess_gas_and_price = Some(BlobExcessGasAndPrice {
                    excess_blob_gas: 0,
                    blob_gasprice: u128::try_from(value).unwrap(),
                });
            }
        })
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace::default());
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(caller)
                .kind(TxKind::Call(entry.address))
                .gas_limit(gas_limit)
                .gas_price(gas_price)
                .data(
                    entry
                        .environment
                        .calldata
                        .exact_bytes(native::product().borrowed())
                        .expect("exact oracle input")
                        .into(),
                )
                .value(entry.environment.value.singleton().unwrap())
                .build()
                .unwrap(),
        )
        .unwrap();
    let kind = if result.result.is_success() {
        OutcomeKind::Return
    } else if matches!(
        result.result,
        revm::context_interface::result::ExecutionResult::Revert { .. }
    ) {
        OutcomeKind::Revert
    } else {
        OutcomeKind::Failure
    };
    let data = result
        .result
        .output()
        .map_or_else(Vec::new, |output| output.to_vec());
    let logs = result
        .result
        .logs()
        .iter()
        .map(|log| {
            (
                log.address,
                log.data
                    .topics()
                    .iter()
                    .map(|topic| U256::from_be_slice(topic.as_slice()))
                    .collect(),
                log.data.data.to_vec(),
            )
        })
        .collect();
    let mut storage = initial_storage;
    let mut balances = BTreeMap::new();
    for (owner, initial) in world.accounts() {
        if *owner == caller {
            continue;
        }
        let final_account = result.state.get(owner);
        balances.insert(
            *owner,
            final_account.map_or_else(|| initial.balance.singleton().unwrap(), |a| a.info.balance),
        );
        if let Some(account) = final_account {
            for (slot, value) in &account.storage {
                storage.insert((*owner, *slot), value.present_value());
            }
        }
    }
    assert_eq!(evm.inspector.depth, 0, "revm call/end trace is unbalanced");
    Observation {
        kind,
        data,
        storage,
        balances,
        logs,
        steps: evm.inspector.steps,
        depth_failures: evm.inspector.depth_failures,
        emitted_logs: evm.inspector.emitted_logs,
    }
}

pub(super) fn unchanged(analysis: &EvmWorldAnalysis<NumericValue>, owners: &[u64]) {
    for outcome in analysis.outcomes() {
        for owner in owners {
            assert_eq!(
                outcome.store.nonce(address(*owner)).singleton(),
                Some(U256::ZERO)
            );
            assert_eq!(outcome.store.existence(address(*owner)), Existence::Present);
            assert_eq!(
                outcome.store.created_in_transaction(address(*owner)),
                Some(false)
            );
            assert_eq!(
                outcome.store.pending_destruction(address(*owner)),
                Some(false)
            );
            assert_eq!(
                outcome
                    .store
                    .read_transient(address(*owner), &word(1), native::product().borrowed())
                    .singleton(),
                Some(U256::ZERO)
            );
        }
    }
}
