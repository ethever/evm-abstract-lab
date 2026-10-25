//! Independent revm execution and journaling, shared by semantic suites.
//! Unknown storage is refined to zero, never selected using abstract membership.
//! Comparisons choose ONE terminal for return data, account effects and may logs.
//! A possible-log map has no order/multiplicity: inclusion cannot prove either.
#![allow(dead_code)]
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
        CallInputs, CallOutcome, CreateInputs, CreateOutcome, InstructionResult, Interpreter,
        interpreter::EthInterpreter, interpreter_types::Jumps,
    },
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub(crate) struct Step {
    pub(crate) code: Address,
    pub(crate) owner: Address,
    pub(crate) caller: Address,
    pub(crate) is_static: bool,
    pub(crate) value: U256,
    pub(crate) code_address: Option<Address>,
    pub(crate) is_init: bool,
    pub(crate) stack: Vec<U256>,
    pub(crate) pc: usize,
    pub(crate) opcode: u8,
    pub(crate) depth: usize,
    pub(crate) storage_write: Option<(U256, U256)>,
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

    fn create(&mut self, _: &mut CTX, _: &mut CreateInputs) -> Option<CreateOutcome> {
        self.depth += 1;
        None
    }

    fn create_end(&mut self, _: &mut CTX, _: &CreateInputs, _: &mut CreateOutcome) {
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
            value: interpreter.input.call_value,
            code_address: interpreter.input.bytecode_address,
            is_init: interpreter.input.bytecode_address.is_none(),
            stack: interpreter.stack.data().clone(),
            pc: interpreter.bytecode.pc(),
            opcode: interpreter.bytecode.opcode(),
            depth: self.depth,
            storage_write: if interpreter.bytecode.opcode() == revm_bytecode::opcode::SSTORE {
                interpreter
                    .stack
                    .peek(0)
                    .ok()
                    .zip(interpreter.stack.peek(1).ok())
            } else {
                None
            },
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

#[derive(Clone, Debug)]
pub(crate) struct AccountEffect {
    pub(crate) exists: bool,
    pub(crate) created: bool,
    pub(crate) touched: bool,
    pub(crate) nonce: u64,
    pub(crate) balance: U256,
    pub(crate) storage: BTreeMap<U256, U256>,
    pub(crate) code: Option<Vec<u8>>,
}

impl AccountEffect {
    fn absent() -> Self {
        Self {
            exists: false,
            created: false,
            touched: false,
            nonce: 0,
            balance: U256::ZERO,
            storage: BTreeMap::new(),
            code: Some(Vec::new()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    pub(crate) kind: OutcomeKind,
    pub(crate) data: Vec<u8>,
    pub(crate) accounts: BTreeMap<Address, AccountEffect>,
    pub(crate) deleted: BTreeSet<Address>,
    pub(crate) storage: BTreeMap<(Address, U256), U256>,
    pub(crate) balances: BTreeMap<Address, U256>,
    pub(crate) logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
    pub(crate) steps: Vec<Step>,
    pub(crate) depth_failures: usize,
    pub(crate) emitted_logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
}

impl Observation {
    pub(crate) fn output_word(&self, offset: usize) -> U256 {
        U256::from_be_slice(&self.data[offset..offset + 32])
    }

    pub(crate) fn slot(&self, owner: u64, slot: u64) -> U256 {
        self.storage[&(
            Address::from_word(U256::from(owner).into()),
            U256::from(slot),
        )]
    }

    // This is coherent component inclusion, not a general existential solver.
    // Selected tests add bounded joint assignments for shared symbolic sources.
    pub(crate) fn is_covered(&self, graph: &EvmWorldAnalysis<NumericValue>) -> bool {
        graph
            .outcomes()
            .iter()
            .any(|outcome| self.outcome_covers(graph, outcome))
    }

    pub(crate) fn outcome_covers(
        &self,
        graph: &EvmWorldAnalysis<NumericValue>,
        outcome: &evm_abstract::analysis::EvmMachineOutcome<NumericValue>,
    ) -> bool {
        let domain = native::product_with(graph.domain_spec());
        outcome.kind == self.kind
            && outcome.data.len().contains(U256::from(self.data.len()))
            && self.data.iter().enumerate().all(|(offset, byte)| outcome.data
                .byte_at(offset, domain.borrowed()).contains(U256::from(*byte)))
            && self.data.as_chunks::<32>().0.iter().enumerate().all(|(i, bytes)| outcome.data
                .read_word(&evm_abstract::domain::EvmValue::constant(U256::from(i * 32)), domain.borrowed())
                .contains(U256::from_be_slice(bytes)))
            && self.storage.iter().all(|((owner, slot), value)| outcome.store
                .read(*owner, &evm_abstract::domain::EvmValue::constant(*slot), domain.borrowed()).contains(*value))
            && self.balances.iter().all(|(owner, value)| outcome.store.read_balance(*owner).contains(*value))
            && self.accounts.iter().all(|(owner, account)| {
                let existence = outcome.store.existence(*owner);
                (existence == Existence::Unknown
                    || (existence == Existence::Present) == account.exists)
                    && outcome.store.nonce(*owner).contains(U256::from(account.nonce))
                    // A missing/Unknown code observation explicitly allows any bytes.
                    && outcome.store.raw_account_code(*owner).is_none_or(|code| Some(code) == account.code)
                    // Public final stores clear transaction-local created/touched
                    // tags (store::clear_transaction); compare final nonce/code
                    // and deletion instead. Trace retains the revm tags.
            })
            && self.deleted.iter().all(|owner| outcome.store.existence(*owner) != Existence::Present
                && outcome.store.raw_account_code(*owner).is_none_or(|code| code.is_empty()))
            // Ordered concrete logs are retained. The abstract may map permits
            // reordering, repetition and additional logs; it proves no exact count.
            && self.logs.iter().all(|(owner, topics, data)| outcome.store.logs_unknown()
                || outcome.store.possible_logs().iter().any(|(site, log)| site.address == *owner
                    && log.topics.len() == topics.len()
                    && log.topics.iter().zip(topics).all(|(value, expected)| value.contains(*expected))
                    && log.data.len().contains(U256::from(data.len()))
                    && data.iter().enumerate().all(|(i, byte)| log.data.byte_at(i, domain.borrowed()).contains(U256::from(*byte)))))
    }

    pub(crate) fn covers(&self, graph: &EvmWorldAnalysis<NumericValue>) {
        assert!(
            self.is_covered(graph),
            "no single abstract terminal covers revm {:?}: data={}, accounts={:?}, storage={:?}, balances={:?}; frontiers={:?}",
            self.kind,
            alloy_primitives::hex::encode(&self.data),
            self.accounts,
            self.storage,
            self.balances,
            graph.frontiers()
        );
    }
}

pub(crate) fn execute(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
) -> Observation {
    execute_gas(world, entry, 10_000_000)
}

pub(crate) fn execute_gas(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    gas_limit: u64,
) -> Observation {
    let mut db = InMemoryDB::default();
    assert!(
        !entry.environment.is_static,
        "static execution needs a STATICCALL wrapper"
    );
    let mut initial_accounts = BTreeMap::new();
    for (owner, account) in world.accounts() {
        if account.existence == Existence::Absent {
            initial_accounts.insert(*owner, AccountEffect::absent());
            continue;
        }
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
        initial_accounts.insert(
            *owner,
            AccountEffect {
                exists: true,
                created: false,
                touched: false,
                nonce: u64::try_from(account.nonce.singleton().unwrap()).unwrap(),
                balance: account.balance.singleton().unwrap(),
                storage: BTreeMap::new(),
                code: Some(world.raw_account_code(*owner).unwrap()),
            },
        );
        // For partially observed worlds this chooses zero for unobserved slots;
        // the abstract input keeps them unknown. It is one concrete witness.
        for (slot, value) in &account.storage {
            let value = value.singleton().expect("concrete observed slot");
            db.insert_account_storage(*owner, *slot, value).unwrap();
            initial_accounts
                .get_mut(owner)
                .unwrap()
                .storage
                .insert(*slot, value);
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
    // Include unexecuted captured accounts, dynamically touched/new accounts,
    // and init frames rolled back before their account reached result.state.
    // Sender nonce/fees and beneficiary gas rewards are transaction settlement,
    // outside this call-execution model. They are the only excluded accounts.
    let beneficiary = environment.coinbase.as_concrete().unwrap_or(Address::ZERO);
    let owners: BTreeSet<_> = initial_accounts
        .keys()
        .copied()
        .chain(result.state.keys().copied())
        .chain(evm.inspector.steps.iter().map(|step| step.owner))
        .filter(|owner| {
            *owner != caller && (*owner != beneficiary || initial_accounts.contains_key(owner))
        })
        .collect();
    let mut accounts = BTreeMap::new();
    let mut deleted = BTreeSet::new();
    for owner in owners {
        let initial = initial_accounts.get(&owner);
        let mut effect = initial.cloned().unwrap_or_else(AccountEffect::absent);
        if let Some(account) = result.state.get(&owner) {
            effect.touched = account.is_touched();
            effect.created = account.is_created();
            if account.is_selfdestructed() {
                deleted.insert(owner);
                effect = AccountEffect::absent();
                // Retain zero-valued cells as explicit deletion witnesses.
                if let Some(initial) = initial {
                    effect
                        .storage
                        .extend(initial.storage.keys().map(|slot| (*slot, U256::ZERO)));
                }
                effect
                    .storage
                    .extend(account.storage.keys().map(|slot| (*slot, U256::ZERO)));
            } else {
                effect.nonce = account.info.nonce;
                effect.balance = account.info.balance;
                effect.code = Some(
                    account
                        .info
                        .code
                        .as_ref()
                        .map(|code| code.original_bytes().to_vec())
                        .or_else(|| initial.and_then(|effect| effect.code.clone()))
                        .unwrap_or_else(|| {
                            assert!(
                                account.info.code_hash == B256::ZERO
                                    || account.info.code_hash == alloy_primitives::keccak256([]),
                                "unresolved concrete code for {owner}"
                            );
                            Vec::new()
                        }),
                );
                effect.exists = effect.nonce != 0
                    || effect.balance != U256::ZERO
                    || !effect.code.as_ref().unwrap().is_empty();
                // EIP-161 deletes touched empty accounts, including sticky touches.
                if !account.is_touched() {
                    effect.exists |= initial.is_some_and(|effect| effect.exists);
                }
                effect.storage.extend(
                    account
                        .storage
                        .iter()
                        .map(|(slot, value)| (*slot, value.present_value())),
                );
            }
        }
        if !effect.exists && initial.is_some_and(|account| account.exists) {
            deleted.insert(owner);
        }
        accounts.insert(owner, effect);
    }
    let storage = accounts
        .iter()
        .flat_map(|(owner, account)| {
            account
                .storage
                .iter()
                .map(move |(slot, value)| ((*owner, *slot), *value))
        })
        .collect();
    let balances = accounts
        .iter()
        .map(|(owner, account)| (*owner, account.balance))
        .collect();
    assert_eq!(evm.inspector.depth, 0, "revm call/end trace is unbalanced");
    Observation {
        kind,
        data,
        accounts,
        deleted,
        storage,
        balances,
        logs,
        steps: evm.inspector.steps,
        depth_failures: evm.inspector.depth_failures,
        emitted_logs: evm.inspector.emitted_logs,
    }
}
