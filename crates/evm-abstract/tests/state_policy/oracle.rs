//! revm owns concrete execution; one abstract outcome must cover the full tuple.

use super::support::{USER, address, word};
use alloy_primitives::{Address, U256};
use evm_abstract::{
    analysis::{OutcomeKind, WorldAnalysis},
    domain::{AbstractValue, Domain},
    world::{Entry, World},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Trace(Vec<(Address, usize)>);

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.0.push((
            interpreter
                .input
                .bytecode_address
                .unwrap_or(interpreter.input.target_address),
            interpreter.bytecode.pc(),
        ));
    }
}

pub(super) struct Witness {
    pub(super) kind: OutcomeKind,
    pub(super) data: Vec<u8>,
    pub(super) storage: BTreeMap<(Address, U256), U256>,
    pub(super) steps: Vec<(Address, usize)>,
    balances: BTreeMap<Address, U256>,
}

impl Witness {
    pub(super) fn output_word(&self, offset: usize) -> U256 {
        U256::from_be_slice(&self.data[offset..offset + 32])
    }

    pub(super) fn covers(&self, graph: &WorldAnalysis) {
        let domain = Domain::from_spec(graph.domain_spec());
        assert!(
            graph.outcomes().iter().any(|outcome| {
                outcome.kind == self.kind
                    && outcome.data.len().contains(U256::from(self.data.len()))
                    && self.data.iter().enumerate().all(|(i, byte)| {
                        outcome.data.byte_at(i, domain).contains(U256::from(*byte))
                    })
                    && self
                        .data
                        .as_chunks::<32>()
                        .0
                        .iter()
                        .enumerate()
                        .all(|(i, bytes)| {
                            outcome
                                .data
                                .read_word(&word((i * 32) as u64), domain)
                                .contains(U256::from_be_slice(bytes))
                        })
                    && self.storage.iter().all(|((owner, slot), n)| {
                        outcome
                            .store
                            .read(*owner, &AbstractValue::constant(*slot), domain)
                            .contains(*n)
                    })
                    && self
                        .balances
                        .iter()
                        .all(|(owner, n)| outcome.store.read_balance(*owner).contains(*n))
            }),
            "no single terminal covers kind={:?}, data={}, store={:?}; frontiers={:?}",
            self.kind,
            alloy_primitives::hex::encode(&self.data),
            self.storage,
            graph.frontiers()
        );
    }
}

pub(super) fn execute(world: &World, entry: &Entry) -> Witness {
    let mut db = InMemoryDB::default();
    let mut storage = BTreeMap::new();
    for (owner, account) in world.accounts() {
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
        for (slot, n) in &account.storage {
            let n = n.singleton().unwrap();
            db.insert_account_storage(*owner, *slot, n).unwrap();
            storage.insert((*owner, *slot), n);
        }
    }
    assert_eq!(entry.environment.caller.as_concrete(), Some(address(USER)));
    db.insert_account_info(
        address(USER),
        AccountInfo {
            balance: U256::MAX,
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
                .caller(address(USER))
                .kind(TxKind::Call(entry.address))
                .gas_limit(10_000_000)
                .value(entry.environment.value.singleton().unwrap())
                .data(entry.environment.calldata.exact_bytes().unwrap().into())
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
        .map_or_else(Vec::new, |data| data.to_vec());
    let mut balances = BTreeMap::new();
    for (owner, account) in world.accounts() {
        let final_account = result.state.get(owner);
        balances.insert(
            *owner,
            final_account.map_or_else(
                || account.balance.singleton().unwrap(),
                |account| account.info.balance,
            ),
        );
        if let Some(account) = final_account {
            for (slot, value) in &account.storage {
                storage.insert((*owner, *slot), value.present_value());
            }
        }
    }
    Witness {
        kind,
        data,
        storage,
        steps: evm.inspector.0,
        balances,
    }
}
