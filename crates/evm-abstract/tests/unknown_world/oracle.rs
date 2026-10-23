//! Finite concrete witnesses use revm's execution and journaling. This module
//! compares complete returned data and account effects from one outcome.
use super::support::{USER, address};
use crate::native;
use alloy_primitives::{Address, U256};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    analysis::{EvmWorldAnalysis, OutcomeKind},
    world::{EvmEntry, EvmWorld, Existence},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(super) struct Step {
    pub(super) code: Address,
    pub(super) owner: Address,
    pub(super) caller: Address,
    pub(super) is_static: bool,
    pub(super) pc: usize,
    pub(super) opcode: u8,
    pub(super) is_init: bool,
}

#[derive(Default)]
struct Trace {
    steps: Vec<Step>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
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
            is_init: interpreter.input.bytecode_address.is_none(),
        });
    }
}

#[derive(Debug)]
pub(super) struct Observation {
    pub(super) kind: OutcomeKind,
    pub(super) data: Vec<u8>,
    pub(super) storage: BTreeMap<(Address, U256), U256>,
    pub(super) balances: BTreeMap<Address, U256>,
    pub(super) logs: Vec<(Address, Vec<U256>, Vec<u8>)>,
    pub(super) steps: Vec<Step>,
    pub(super) deleted: BTreeSet<Address>,
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
            self.is_covered(analysis),
            "no single abstract outcome covers concrete {:?} data={} storage={:?} balances={:?}; status={:?} frontiers={:?}",
            self.kind,
            alloy_primitives::hex::encode(&self.data),
            self.storage,
            self.balances,
            analysis.status(),
            super::support::frontier_causes(analysis)
        );
    }

    pub(super) fn is_covered(&self, analysis: &EvmWorldAnalysis<NumericValue>) -> bool {
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
                && self
                    .balances
                    .iter()
                    .all(|(owner, balance)| outcome.store.read_balance(*owner).contains(*balance))
                && self.deleted.iter().all(|owner| {
                    outcome.store.existence(*owner) != Existence::Present
                        && outcome
                            .store
                            .raw_account_code(*owner)
                            .is_none_or(|code| code.is_empty())
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
                                    .all(|(possible, concrete)| possible.contains(*concrete))
                                && log.data.len().contains(U256::from(data.len()))
                                && data.iter().enumerate().all(|(offset, byte)| {
                                    log.data
                                        .byte_at(offset, native::product().borrowed())
                                        .contains(U256::from(*byte))
                                })
                        })
                })
        })
    }
}

pub(super) fn execute(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
) -> Observation {
    execute_with_gas(world, entry, 10_000_000)
}

pub(super) fn execute_with_gas(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    gas_limit: u64,
) -> Observation {
    let mut db = InMemoryDB::default();
    let mut initial_storage = BTreeMap::new();
    for (owner, account) in world.accounts() {
        if account.existence == Existence::Absent {
            continue;
        }
        let code = Bytecode::new_raw(Bytes::from(
            world
                .raw_account_code(*owner)
                .expect("concrete oracle code"),
        ));
        let balance = account
            .balance
            .singleton()
            .expect("concrete oracle balance");
        let nonce =
            u64::try_from(account.nonce.singleton().expect("concrete oracle nonce")).unwrap();
        db.insert_account_info(
            *owner,
            AccountInfo::new(balance, nonce, code.hash_slow(), code),
        );
        for (slot, value) in &account.storage {
            let value = value.singleton().expect("concrete oracle slot");
            db.insert_account_storage(*owner, *slot, value).unwrap();
            initial_storage.insert((*owner, *slot), value);
        }
    }
    db.insert_account_info(
        address(USER),
        AccountInfo {
            balance: U256::from(100_000_000),
            ..AccountInfo::default()
        },
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id()))
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace::default());
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(entry.environment.caller.as_concrete().unwrap())
                .kind(TxKind::Call(entry.address))
                .gas_limit(gas_limit)
                .data(
                    entry
                        .environment
                        .calldata
                        .exact_bytes(native::product().borrowed())
                        .expect("exact oracle calldata")
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
    let deleted: BTreeSet<_> = result
        .state
        .iter()
        .filter(|(_, account)| account.is_selfdestructed())
        .map(|(owner, _)| *owner)
        .collect();
    for (owner, initial) in world.accounts() {
        if *owner == address(USER) {
            continue;
        }
        let final_account = result.state.get(owner);
        balances.insert(
            *owner,
            final_account.map_or_else(
                || initial.balance.singleton().unwrap(),
                |account| account.info.balance,
            ),
        );
        if let Some(account) = final_account {
            for (slot, value) in &account.storage {
                storage.insert(
                    (*owner, *slot),
                    if deleted.contains(owner) {
                        U256::ZERO
                    } else {
                        value.present_value()
                    },
                );
            }
        }
    }
    assert!(
        !evm.inspector.steps.is_empty(),
        "oracle fixture did not execute runtime bytecode"
    );
    Observation {
        kind,
        data,
        storage,
        balances,
        logs,
        steps: evm.inspector.steps,
        deleted,
    }
}

pub(super) fn concrete_unknown(
    world: &EvmWorld<NumericValue>,
    owner: u64,
    code: &super::support::Fixture,
) -> EvmWorld<NumericValue> {
    // World observations are immutable. Build an independent refinement rather
    // than replacing an existing Unknown observation through World::insert.
    let mut concrete = EvmWorld::<NumericValue>::new(world.fork(), world.provenance());
    for (address, account) in world.accounts() {
        if *address != super::support::address(owner) {
            concrete.insert(*address, account.clone()).unwrap();
        }
    }
    let mut account = evm_abstract::world::EvmAccount::<NumericValue>::from_hex(
        &alloy_primitives::hex::encode(&code.bytes),
        world.fork(),
    )
    .unwrap();
    account.balance = super::support::word(1_000_000);
    if let Some(observed) = world.account(super::support::address(owner)) {
        if let Some(balance) = observed.balance.singleton() {
            account.balance = evm_abstract::domain::EvmValue::<NumericValue>::constant(balance);
        }
        if let Some(nonce) = observed.nonce.singleton() {
            account.nonce = evm_abstract::domain::EvmValue::<NumericValue>::constant(nonce);
        }
        account.storage.clone_from(&observed.storage);
    }
    concrete
        .insert(super::support::address(owner), account)
        .unwrap();
    concrete
}
