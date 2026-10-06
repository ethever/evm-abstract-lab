//! Independent concrete execution and a joint output/effects coverage predicate.
use alloy_primitives::{Address, U256};
use evm_abstract::{
    analysis::{OutcomeKind, WorldAnalysis},
    domain::{Domain, Value},
    world::{Entry, World},
};
use revm::{
    Context, ExecuteEvm, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};

pub(super) fn compare(world: &World, entry: &Entry, analysis: &WorldAnalysis) -> OutcomeKind {
    let mut db = InMemoryDB::default();
    for (address, account) in world.accounts() {
        if account.existence == evm_abstract::world::Existence::Absent {
            continue;
        }
        let bytes = world.raw_account_code(*address).unwrap();
        let code = Bytecode::new_raw(Bytes::from(bytes));
        let balance = *account.balance.constants().unwrap().iter().next().unwrap();
        let nonce = account
            .nonce
            .constants()
            .and_then(|values| values.iter().next())
            .copied()
            .unwrap_or(U256::ZERO);
        db.insert_account_info(
            *address,
            AccountInfo::new(
                balance,
                u64::try_from(nonce).unwrap(),
                code.hash_slow(),
                code,
            ),
        );
        for (slot, value) in &account.storage {
            db.insert_account_storage(
                *address,
                *slot,
                *value.constants().unwrap().iter().next().unwrap(),
            )
            .unwrap();
        }
    }
    db.insert_account_info(
        entry.environment.caller.as_concrete().unwrap(),
        AccountInfo {
            balance: U256::from(100_000_000),
            ..AccountInfo::default()
        },
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id()))
        .with_db(db);
    let mut evm = context.build_mainnet();
    let result = evm
        .transact(
            TxEnv::builder()
                .caller(entry.environment.caller.as_concrete().unwrap())
                .kind(TxKind::Call(entry.address))
                .gas_limit(10_000_000)
                .data(
                    entry
                        .environment
                        .calldata
                        .exact_bytes()
                        .expect("oracle requires exact calldata")
                        .into(),
                )
                .value(
                    *entry
                        .environment
                        .value
                        .constants()
                        .expect("oracle requires exact value")
                        .iter()
                        .next()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let concrete_kind = if result.result.is_success() {
        OutcomeKind::Return
    } else if matches!(
        result.result,
        revm::context_interface::result::ExecutionResult::Revert { .. }
    ) {
        OutcomeKind::Revert
    } else {
        OutcomeKind::Failure
    };
    let output = result.result.output().map_or(&[][..], |data| data.as_ref());
    assert!(
        analysis.outcomes().iter().any(|outcome| {
            outcome.kind == concrete_kind
                && outcome.data.len().contains(U256::from(output.len()))
                && output.iter().enumerate().all(|(offset, byte)| {
                    outcome
                        .data
                        .byte_at(offset, Domain::default())
                        .contains(U256::from(*byte))
                })
                && result.state.iter().all(|(owner, account)| {
                    if world.account(*owner).is_none()
                        && outcome.store.created_in_transaction(*owner) != Some(true)
                    {
                        return true;
                    }
                    account.storage.iter().all(|(slot, value)| {
                        outcome
                            .store
                            .read(*owner, &Value::constant(*slot), Domain::default())
                            .contains(value.present_value())
                    }) && outcome
                        .store
                        .read_balance(*owner)
                        .contains(account.info.balance)
                })
                && result.result.logs().iter().all(|log| {
                    outcome.store.logs_unknown()
                        || outcome
                            .store
                            .possible_logs()
                            .iter()
                            .any(|(site, possible)| {
                                site.address == log.address
                                    && possible.topics.len() == log.data.topics().len()
                                    && possible.topics.iter().zip(log.data.topics()).all(
                                        |(abstract_topic, concrete)| {
                                            abstract_topic
                                                .contains(U256::from_be_slice(concrete.as_slice()))
                                        },
                                    )
                                    && possible
                                        .data
                                        .len()
                                        .contains(U256::from(log.data.data.len()))
                                    && log.data.data.iter().enumerate().all(|(offset, byte)| {
                                        possible
                                            .data
                                            .byte_at(offset, Domain::default())
                                            .contains(U256::from(*byte))
                                    })
                            })
                })
        }),
        "no single abstract outcome jointly covers concrete {result:?}; abstract={:?}",
        analysis.outcomes()
    );
    concrete_kind
}

pub(super) fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
