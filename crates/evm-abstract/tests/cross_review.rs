//! Independent regressions for fork dispatch, retained frontier identity and work.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, FrontierReason, OutcomeKind, Status, analyze_world},
    domain::{Domain, Value},
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter},
    primitives::{Bytes, TxKind, hex},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeSet;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn entry(number: u64) -> Entry {
    Entry {
        address: address(number),
        caller: address(0x900),
        value: Value::constant(U256::ZERO),
        calldata: ByteArray::empty(),
        is_static: false,
    }
}

fn world(fork: Fork, code: &str) -> World {
    let mut world = World::new(fork, "review:independent");
    world
        .insert(address(0x101), Account::from_hex(code, fork).unwrap())
        .unwrap();
    world
}

#[test]
fn osaka_p256_entry_is_a_native_precompile_frontier() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = World::new(fork, "review:precompile");
        world
            .insert(address(0x100), Account::from_hex("00", fork).unwrap())
            .unwrap();
        let analysis = analyze_world(world, entry(0x100), ExecutionConfig::default()).unwrap();
        if fork == Fork::Osaka {
            assert_eq!(analysis.status(), Status::Incomplete);
            assert!(
                analysis.frontiers().iter().any(|frontier| {
                    frontier.reason == FrontierReason::Precompile(address(0x100))
                })
            );
        } else {
            assert_eq!(analysis.status(), Status::Converged);
        }
    }
}

#[test]
fn model_frontier_key_retains_the_current_stack_height() {
    // SELFDESTRUCT consumes beneficiary zero; retained 42 remains in the frame.
    let analysis = analyze_world(
        world(Fork::Osaka, "602a5fff"),
        entry(0x101),
        ExecutionConfig::default(),
    )
    .unwrap();
    let frontier = analysis
        .frontiers()
        .iter()
        .find(|frontier| matches!(frontier.reason, FrontierReason::UnsupportedOpcode(0xff)))
        .unwrap();
    let state = &analysis.states()[frontier.from.unwrap()];
    let payload = state.exit.as_ref().unwrap();
    assert_eq!(payload.active().stack.len(), 1);
    assert_eq!(
        frontier
            .target
            .as_ref()
            .unwrap()
            .frames
            .last()
            .unwrap()
            .stack_height,
        payload.active().stack.len()
    );
}

#[test]
fn external_code_hash_scanning_uses_the_shared_work_budget() {
    let mut world = world(Fork::Osaka, "6102003f00");
    world
        .insert(
            address(0x200),
            Account::from_hex(&"5f".repeat(10_000), Fork::Osaka).unwrap(),
        )
        .unwrap();
    let analysis = analyze_world(
        world,
        entry(0x101),
        ExecutionConfig {
            max_work: 1_000,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(analysis.work() <= 1_000);
}

#[derive(Default)]
struct Visited(BTreeSet<Address>);

impl<CTX> Inspector<CTX, EthInterpreter> for Visited {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.0.insert(interpreter.input.target_address);
    }
}

#[test]
fn outer_revert_restores_committed_grandchild_effects_and_preserves_root_events() {
    let root = "60445f5360a160015fa160035f5560115f5d5f5f5f5f6007610200620f4240f15060a260015fa100";
    let middle = "5f5f5f5f6005610300620f4240f15060075f5560085f5d60b25f5fa15f5ffd";
    let child = "60095f55600a5f5d60c35f5fa100";
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = World::new(fork, "review:deep-rollback");
        let mut db = InMemoryDB::default();
        for (number, code, balance, initial_slot) in [
            (0x101, root, 1_000_000_u64, 0_u64),
            (0x200, middle, 100, 2),
            (0x300, child, 10, 4),
        ] {
            let owner = address(number);
            let mut account = Account::from_hex(code, fork).unwrap();
            account.balance = Value::constant(U256::from(balance));
            account
                .storage
                .insert(U256::ZERO, Value::constant(U256::from(initial_slot)));
            world.insert(owner, account).unwrap();
            let bytecode = Bytecode::new_raw(Bytes::from(hex::decode(code).unwrap()));
            db.insert_account_info(
                owner,
                AccountInfo::new(U256::from(balance), 0, bytecode.hash_slow(), bytecode),
            );
            db.insert_account_storage(owner, U256::ZERO, U256::from(initial_slot))
                .unwrap();
        }
        db.insert_account_info(
            address(0x900),
            AccountInfo {
                balance: U256::from(100_000_000),
                ..AccountInfo::default()
            },
        );
        let analysis = analyze_world(world, entry(0x101), ExecutionConfig::default()).unwrap();
        assert_eq!(analysis.status(), Status::Converged);
        ssa::build_world(&analysis)
            .unwrap()
            .verify(&analysis)
            .unwrap();
        assert!(
            analysis
                .states()
                .iter()
                .any(|state| state.key.frames.len() == 3)
        );
        let context = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
            .with_db(db);
        let mut evm = context.build_mainnet_with_inspector(Visited::default());
        let concrete = evm
            .inspect_tx(
                TxEnv::builder()
                    .caller(address(0x900))
                    .kind(TxKind::Call(address(0x101)))
                    .gas_limit(10_000_000)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(concrete.result.is_success());
        assert_eq!(
            evm.inspector.0,
            BTreeSet::from([address(0x101), address(0x200), address(0x300)])
        );
        let logs = concrete.result.logs();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().all(|log| log.address == address(0x101)));
        let domain = Domain::default();
        let returns: Vec<_> = analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect();
        assert!(!returns.is_empty());
        for outcome in &returns {
            assert_eq!(
                outcome
                    .store
                    .read(address(0x101), &Value::constant(U256::ZERO), domain),
                Value::constant(U256::from(3))
            );
            assert_eq!(
                outcome
                    .store
                    .read_transient(address(0x101), &Value::constant(U256::ZERO), domain),
                Value::constant(U256::from(17))
            );
            assert_eq!(
                outcome
                    .store
                    .read(address(0x200), &Value::constant(U256::ZERO), domain),
                Value::constant(U256::from(2))
            );
            assert_eq!(
                outcome
                    .store
                    .read(address(0x300), &Value::constant(U256::ZERO), domain),
                Value::constant(U256::from(4))
            );
            assert_eq!(
                outcome
                    .store
                    .read_transient(address(0x300), &Value::constant(U256::ZERO), domain),
                Value::constant(U256::ZERO)
            );
            assert_eq!(outcome.store.possible_logs().len(), 2);
            assert!(
                outcome
                    .store
                    .possible_logs()
                    .keys()
                    .all(|key| key.address == address(0x101) && key.code_address == address(0x101))
            );
        }
        assert!(returns.iter().any(|outcome| {
            concrete
                .state
                .iter()
                .filter(|(owner, _)| analysis.world().account(**owner).is_some())
                .all(|(owner, account)| {
                    outcome
                        .store
                        .read_balance(*owner)
                        .contains(account.info.balance)
                        && account.storage.iter().all(|(slot, value)| {
                            outcome
                                .store
                                .read(*owner, &Value::constant(*slot), domain)
                                .contains(value.present_value())
                        })
                })
                && logs.iter().all(|log| {
                    outcome.store.possible_logs().iter().any(|(key, observed)| {
                        key.address == log.address
                            && observed.topics.len() == log.data.topics().len()
                            && observed.topics.iter().zip(log.data.topics()).all(
                                |(topic, concrete)| {
                                    topic.contains(U256::from_be_slice(concrete.as_slice()))
                                },
                            )
                            && observed.data.exact_bytes() == Some(log.data.data.to_vec())
                    })
                })
        }));
    }
}
