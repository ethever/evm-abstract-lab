//! Finite CREATE/CREATE2 fixtures exercise the same transaction machine as calls.
//! The revm oracle below uses the pinned upstream interpreter and observes actual
//! initcode entry, deployed-code calls, nonce changes, and SELFDESTRUCT.

use alloy_primitives::{Address, U256, keccak256};
use evm_abstract::{
    Fork,
    analysis::{
        self, CreationBoundary, ExecutionConfig, FrontierReason, OutcomeKind, Status, WorldAnalysis,
    },
    domain::{Domain, Value},
    world::{Account, ByteArray, Entry, Existence, Store, World},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::{TxEnv, result::ExecutionResult},
    database::InMemoryDB,
    interpreter::{
        CreateInputs, CreateOutcome, Interpreter, interpreter::EthInterpreter,
        interpreter_types::Jumps,
    },
    primitives::{Bytes, TxKind, hex},
    state::{AccountInfo, Bytecode},
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn word(number: u64) -> Value {
    Value::constant(U256::from(number))
}
fn finite_contains(value: Value, expected: u64) -> bool {
    value
        .constants()
        .is_some_and(|values| values.contains(&U256::from(expected)))
}

fn code(bytes: &[u8], fork: Fork) -> Account {
    Account::from_hex(&hex::encode(bytes), fork).unwrap()
}
fn push2(bytes: &mut Vec<u8>, number: usize) {
    bytes.extend([0x61, (number >> 8) as u8, number as u8]);
}

fn init(runtime: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    let offset = prefix.len() + 13;
    push2(&mut bytes, runtime.len());
    push2(&mut bytes, offset);
    bytes.extend([0x5f, 0x39]);
    push2(&mut bytes, runtime.len());
    bytes.extend([0x5f, 0xf3]);
    assert_eq!(bytes.len(), offset);
    bytes.extend(runtime);
    bytes
}

fn factory(initcode: &[u8], create2: bool, value: u8, post: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push2(&mut bytes, initcode.len());
    push2(&mut bytes, 0);
    bytes.extend([0x5f, 0x39]);
    if create2 {
        bytes.extend([0x60, 5]);
    }
    push2(&mut bytes, initcode.len());
    bytes.extend([0x5f, 0x60, value, if create2 { 0xf5 } else { 0xf0 }]);
    bytes.extend(post);
    let offset = bytes.len();
    bytes[4] = (offset >> 8) as u8;
    bytes[5] = offset as u8;
    bytes.extend(initcode);
    bytes
}

fn call_created() -> Vec<u8> {
    // Stack retains the CREATE result until it is stored at factory slot zero.
    hex::decode("805f555060205f5f5f5f5f5462fffffff160015560205ff3").unwrap()
}

fn return_address() -> Vec<u8> {
    hex::decode("805f555f5260205ff3").unwrap()
}

fn fixture(fork: Fork, factory: &[u8], initcode: &[u8], create2: bool) -> (World, Address) {
    let creator = address(0x101);
    let destination = if create2 {
        creator.create2_from_code(U256::from(5).to_be_bytes::<32>(), initcode)
    } else {
        creator.create(0)
    };
    let mut world = World::new(fork, "finite creation fixture");
    let mut account = code(factory, fork);
    account.balance = word(100);
    world.insert(creator, account).unwrap();
    world.insert(destination, Account::absent()).unwrap();
    world.insert(Address::ZERO, Account::absent()).unwrap();
    world.insert(address(0x200), Account::empty()).unwrap();
    (world, destination)
}

fn run(world: World) -> WorldAnalysis {
    let config = ExecutionConfig {
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    };
    analysis::analyze_world(
        world,
        Entry {
            address: address(0x101),
            caller: address(0x1000),
            value: word(0),
            calldata: ByteArray::empty(),
            is_static: false,
        },
        config,
    )
    .unwrap()
}

fn returned(analysis: &WorldAnalysis) -> impl Iterator<Item = &Store> {
    analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| &outcome.store)
}

#[derive(Default, Debug)]
struct Trace {
    creates: usize,
    init_steps: usize,
    runtime_steps: usize,
    destroyed: usize,
    runtime_address: Address,
}
impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn create(&mut self, _: &mut CTX, _: &mut CreateInputs) -> Option<CreateOutcome> {
        self.creates += 1;
        None
    }
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        if interpreter.input.bytecode_address.is_none() {
            self.init_steps += 1;
        }
        if interpreter.input.bytecode_address == Some(self.runtime_address) {
            self.runtime_steps += 1;
        }
        if interpreter.bytecode.opcode() == 0xff {
            self.destroyed += 1;
        }
    }
}

fn oracle(
    world: World,
    destination: Address,
    expected_init: bool,
    expected_runtime: bool,
    expected_destroy: bool,
) -> WorldAnalysis {
    oracle_expect_creation(
        world,
        destination,
        expected_init,
        expected_runtime,
        expected_destroy,
        true,
    )
}

fn oracle_expect_creation(
    world: World,
    destination: Address,
    expected_init: bool,
    expected_runtime: bool,
    expected_destroy: bool,
    expected_create: bool,
) -> WorldAnalysis {
    let mut db = InMemoryDB::default();
    for (owner, account) in world.accounts() {
        if account.existence == Existence::Absent {
            continue;
        }
        let bytecode = Bytecode::new_raw(Bytes::from(world.raw_account_code(*owner).unwrap()));
        db.insert_account_info(
            *owner,
            AccountInfo::new(
                *account.balance.constants().unwrap().first().unwrap(),
                account
                    .nonce
                    .constants()
                    .unwrap()
                    .first()
                    .unwrap()
                    .to::<u64>(),
                bytecode.hash_slow(),
                bytecode,
            ),
        );
        for (slot, value) in &account.storage {
            db.insert_account_storage(*owner, *slot, *value.constants().unwrap().first().unwrap())
                .unwrap();
        }
    }
    db.insert_account_info(
        address(0x1000),
        AccountInfo {
            balance: U256::from(100_000_000),
            ..AccountInfo::default()
        },
    );
    let fork = world.fork();
    let analysis = run(world);
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{fork}: {:?}",
        analysis.frontiers()
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace {
        runtime_address: destination,
        ..Trace::default()
    });
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(address(0x1000))
                .kind(TxKind::Call(address(0x101)))
                .gas_limit(10_000_000)
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        evm.inspector.creates > 0,
        expected_create,
        "oracle did not execute CREATE: {:?}",
        evm.inspector
    );
    assert_eq!(
        evm.inspector.init_steps > 0,
        expected_init,
        "{:?}",
        evm.inspector
    );
    assert_eq!(
        evm.inspector.runtime_steps > 0,
        expected_runtime,
        "{:?}",
        evm.inspector
    );
    assert_eq!(
        evm.inspector.destroyed > 0,
        expected_destroy,
        "{:?}",
        evm.inspector
    );
    let expected_kind = if result.result.is_success() {
        OutcomeKind::Return
    } else if matches!(result.result, ExecutionResult::Revert { .. }) {
        OutcomeKind::Revert
    } else {
        OutcomeKind::Failure
    };
    let output = result
        .result
        .output()
        .map(|bytes| bytes.as_ref())
        .unwrap_or(&[]);
    let matching = analysis.outcomes().iter().any(|outcome| {
        outcome.kind == expected_kind
            && outcome.data.len().contains(U256::from(output.len()))
            && output.iter().enumerate().all(|(index, byte)| {
                outcome
                    .data
                    .byte_at(index, Domain::default())
                    .contains(U256::from(*byte))
            })
            && result
                .state
                .iter()
                .filter(|(owner, _)| analysis.world().account(**owner).is_some())
                .all(|(owner, account)| {
                    if account.is_selfdestructed() {
                        return outcome.store.existence(*owner) == Existence::Absent
                            && outcome.store.raw_account_code(*owner) == Some(Vec::new())
                            && outcome.store.nonce(*owner).contains(U256::ZERO)
                            && outcome.store.read_balance(*owner).contains(U256::ZERO);
                    }
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
                                .read(*owner, &Value::constant(*slot), Domain::default())
                                .contains(value.present_value())
                        })
                        && account.info.code.as_ref().is_none_or(|code| {
                            outcome.store.raw_account_code(*owner).as_deref()
                                == Some(code.original_bytes().as_ref())
                        })
                })
    });
    assert!(
        matching,
        "concrete output/state missed jointly\nresult={result:?}\nabstract={:?}\ntrace={:?}",
        analysis.outcomes(),
        evm.inspector
    );
    evm_abstract::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
    analysis
}

#[test]
fn creation_and_subsequent_runtime_call_match_revm_for_all_supported_forks() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for create2 in [false, true] {
            let runtime = hex::decode("602a5f5260205ff3").unwrap();
            let initcode = init(
                &runtime,
                &hex::decode("305f55336001553460025536600355").unwrap(),
            );
            let bytes = factory(&initcode, create2, 7, &call_created());
            let (world, destination) = fixture(fork, &bytes, &initcode, create2);
            let analysis = oracle(world, destination, true, true, false);
            assert!(returned(&analysis).any(|store| {
                store.raw_account_code(destination) == Some(runtime.clone())
                    && store.nonce(address(0x101)) == word(1)
                    && store.nonce(destination) == word(1)
                    && store.read_balance(destination) == word(7)
                    && store
                        .read(destination, &word(0), Domain::default())
                        .contains(U256::from_be_slice(destination.as_slice()))
                    && store.read(destination, &word(1), Domain::default()) == word(0x101)
                    && store.read(destination, &word(2), Domain::default()) == word(7)
                    && store.read(destination, &word(3), Domain::default()) == word(0)
            }));
        }
    }
}

#[test]
fn collision_balance_prefunding_and_nonce_rejection_match_revm() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = init(&[0x00], &[]);
        let bytes = factory(&initcode, false, 7, &return_address());
        for collision in [0, 1, 2] {
            let (mut world, destination) = fixture(fork, &bytes, &initcode, false);
            // Each fixture starts with an explicit observation, so use a fresh world.
            let mut target = if collision == 2 {
                code(&[0], fork)
            } else {
                Account::empty()
            };
            target.balance = word(3);
            if collision == 1 {
                target.nonce = word(1);
            }
            let mut replaced = World::new(fork, "prefund or collision");
            for (owner, account) in world.accounts() {
                if *owner != destination {
                    replaced.insert(*owner, account.clone()).unwrap();
                }
            }
            replaced.insert(destination, target).unwrap();
            world = replaced;
            let analysis = oracle(world, destination, collision == 0, false, false);
            assert!(
                returned(&analysis).any(|store| store.nonce(address(0x101)) == word(1)
                    && store.read_balance(destination)
                        == word(if collision == 0 { 10 } else { 3 }))
            );
        }
    }
}

#[test]
fn insufficient_balance_and_maximum_sender_nonce_reject_before_nonce_bump() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = init(&[0], &[]);
        let bytes = factory(&initcode, false, 7, &return_address());
        for maximum_nonce in [false, true] {
            let (world, destination) = fixture(fork, &bytes, &initcode, false);
            let mut modified = World::new(fork, "creation rejects before nonce bump");
            for (owner, account) in world.accounts() {
                let mut account = account.clone();
                if *owner == address(0x101) {
                    if maximum_nonce {
                        account.nonce = Value::constant(U256::from(u64::MAX));
                    } else {
                        account.balance = word(6);
                    }
                }
                modified.insert(*owner, account).unwrap();
            }
            let analysis = oracle(modified, destination, false, false, false);
            assert!(returned(&analysis).all(|store| store.nonce(address(0x101))
                == Value::constant(U256::from(if maximum_nonce { u64::MAX } else { 0 }))
                && store.existence(destination) == Existence::Absent));
        }
    }
}

#[test]
fn finite_nonce_alternatives_preserve_distinct_deployed_code_versions() {
    let initcode = init(&[0], &[]);
    let bytes = factory(&initcode, false, 0, &return_address());
    let mut world = World::new(Fork::Osaka, "finite creator nonces");
    let mut creator = code(&bytes, Fork::Osaka);
    creator.nonce = Domain::default().join(&word(0), &word(1));
    world.insert(address(0x101), creator).unwrap();
    for nonce in [0, 1] {
        world
            .insert(address(0x101).create(nonce), Account::absent())
            .unwrap();
    }
    let analysis = run(world);
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    for nonce in [0, 1] {
        assert!(
            returned(&analysis)
                .any(|store| store.raw_account_code(address(0x101).create(nonce)) == Some(vec![0]))
        );
    }
}

#[test]
fn unobserved_nonce_endowment_and_runtime_are_explicit_creation_boundaries() {
    let initcode = init(&[0], &[]);
    let bytes = factory(&initcode, false, 0, &return_address());
    let (world, _) = fixture(Fork::Osaka, &bytes, &initcode, false);
    let mut unknown_nonce = World::new(Fork::Osaka, "unknown creator nonce");
    for (owner, account) in world.accounts() {
        let mut account = account.clone();
        if *owner == address(0x101) {
            account.nonce = Value::top();
        }
        unknown_nonce.insert(*owner, account).unwrap();
    }
    let analysis = run(unknown_nonce);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.frontiers().iter().any(|frontier| frontier.reason
        == FrontierReason::Creation(CreationBoundary::UnknownNonce(address(0x101)))));

    let mut world = World::new(Fork::Osaka, "unknown endowment");
    world
        .insert(
            address(0x101),
            Account::from_hex("5f5f5f35f000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let analysis = analysis::analyze_world(
        world,
        Entry {
            address: address(0x101),
            caller: address(0x1000),
            value: word(0),
            calldata: ByteArray::unknown(),
            is_static: false,
        },
        ExecutionConfig::default(),
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis.frontiers().iter().any(|frontier| frontier.reason
            == FrontierReason::Creation(CreationBoundary::UnknownEndowment))
    );

    let initcode = hex::decode("425f5260205ff3").unwrap();
    let bytes = factory(&initcode, false, 0, &return_address());
    let (world, _) = fixture(Fork::Osaka, &bytes, &initcode, false);
    let analysis = run(world);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.frontiers().iter().any(|frontier| frontier.reason
        == FrontierReason::Creation(CreationBoundary::UnknownRuntimeCode)));
}

#[test]
fn unrepresentable_nonce_and_shared_creation_limits_are_explicit() {
    let initcode = init(&[0], &[]);
    let bytes = factory(&initcode, false, 0, &return_address());
    let (world, _) = fixture(Fork::Osaka, &bytes, &initcode, false);
    let mut invalid_nonce = World::new(Fork::Osaka, "nonce outside protocol representation");
    for (owner, account) in world.accounts() {
        let mut account = account.clone();
        if *owner == address(0x101) {
            account.nonce = Value::constant(U256::MAX);
        }
        invalid_nonce.insert(*owner, account).unwrap();
    }
    let analysis = run(invalid_nonce);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.frontiers().iter().any(|frontier| frontier.reason
        == FrontierReason::Creation(CreationBoundary::NonceOverflow(address(0x101)))));
    for (config, reason) in [
        (
            ExecutionConfig {
                max_call_depth: 1,
                ..ExecutionConfig::default()
            },
            FrontierReason::CallDepth,
        ),
        (
            ExecutionConfig {
                max_work: 1,
                ..ExecutionConfig::default()
            },
            FrontierReason::Work,
        ),
        (
            ExecutionConfig {
                max_memory_bytes: 1,
                ..ExecutionConfig::default()
            },
            FrontierReason::Memory,
        ),
    ] {
        let analysis = analysis::analyze_world(
            world.clone(),
            Entry {
                address: address(0x101),
                caller: address(0x1000),
                value: word(0),
                calldata: ByteArray::empty(),
                is_static: false,
            },
            config,
        )
        .unwrap();
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(
            analysis
                .frontiers()
                .iter()
                .any(|frontier| frontier.reason == reason)
        );
    }
}

#[test]
fn a_known_collision_fact_does_not_require_the_other_account_observation() {
    let initcode = init(&[0], &[]);
    let bytes = factory(&initcode, false, 0, &return_address());
    let (world, destination) = fixture(Fork::Osaka, &bytes, &initcode, false);
    for observed_code in [false, true] {
        let mut modified = World::new(Fork::Osaka, "one collision fact is sufficient");
        for (owner, account) in world.accounts() {
            if *owner != destination {
                modified.insert(*owner, account.clone()).unwrap();
            }
        }
        let mut target = if observed_code {
            code(&[0], Fork::Osaka)
        } else {
            Account::unknown()
        };
        target.nonce = if observed_code { Value::top() } else { word(1) };
        modified.insert(destination, target).unwrap();
        let analysis = run(modified);
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        assert!(
            returned(&analysis)
                .all(|store| store.created_in_transaction(destination) == Some(false))
        );
        assert!(returned(&analysis).any(|store| store.nonce(address(0x101)) == word(1)));
    }
}

#[test]
fn failed_init_and_outer_revert_restore_creation_effects_at_the_correct_savepoint() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = hex::decode("602a5f5560075f5360015ffd").unwrap();
        let bytes = factory(&initcode, false, 7, &return_address());
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let analysis = oracle(world, destination, true, false, false);
        assert!(
            returned(&analysis).any(|store| store.nonce(address(0x101)) == word(1)
                && store.existence(destination) == Existence::Absent
                && store.read_balance(address(0x101)) == word(100)
                && store.read(destination, &word(0), Domain::default()) == word(0))
        );
        let initcode = init(&[0], &hex::decode("602a5f55").unwrap());
        let bytes = factory(&initcode, false, 7, &hex::decode("505f5ffd").unwrap());
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let analysis = oracle(world, destination, true, false, false);
        assert!(
            analysis
                .outcomes()
                .iter()
                .filter(|outcome| outcome.kind == OutcomeKind::Revert)
                .all(|outcome| outcome.store.nonce(address(0x101)) == word(0)
                    && outcome.store.existence(destination) == Existence::Absent
                    && outcome.store.raw_account_code(destination) == Some(Vec::new()))
        );
    }
}

#[test]
fn create_revert_exposes_revert_data_while_success_exposes_no_runtime_data() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = hex::decode("60075f5360015ffd").unwrap();
        let bytes = factory(
            &initcode,
            false,
            7,
            &hex::decode("503d6001553d5f5f3e60205ff3").unwrap(),
        );
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let analysis = oracle(world, destination, true, false, false);
        assert!(returned(&analysis).any(|store| finite_contains(
            store.read(address(0x101), &word(1), Domain::default()),
            1
        ) && finite_contains(
            store.nonce(address(0x101)),
            1
        )));

        let initcode = init(&[0], &[]);
        let bytes = factory(
            &initcode,
            false,
            0,
            &hex::decode("803d6001555f5260205ff3").unwrap(),
        );
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let analysis = oracle(world, destination, true, false, false);
        assert!(returned(&analysis).all(|store| store.read(
            address(0x101),
            &word(1),
            Domain::default()
        ) == word(0)));
    }
}

#[test]
fn empty_invalid_and_oversize_runtime_have_real_creation_semantics() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for runtime in [
            vec![],
            vec![0xef],
            vec![0xef, 0, 1],
            vec![0; 24_576],
            vec![0; 24_577],
        ] {
            let initcode = init(&runtime, &[]);
            let bytes = factory(&initcode, false, 0, &return_address());
            let (world, destination) = fixture(fork, &bytes, &initcode, false);
            let analysis = oracle(world, destination, true, false, false);
            assert!(
                returned(&analysis).any(|store| store.nonce(address(0x101)) == word(1)
                    && store.existence(destination)
                        == if runtime.first() != Some(&0xef) && runtime.len() <= 24_576 {
                            Existence::Present
                        } else {
                            Existence::Absent
                        })
            );
        }
    }
}

#[test]
fn eip3860_initcode_size_boundary_matches_the_selected_forks() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for size in [49_152, 49_153] {
            let initcode = vec![0; size];
            let bytes = factory(&initcode, false, 0, &return_address());
            let (world, destination) = fixture(fork, &bytes, &initcode, false);
            let analysis = oracle_expect_creation(
                world,
                destination,
                size == 49_152,
                false,
                false,
                size == 49_152,
            );
            if size == 49_153 {
                assert!(
                    analysis
                        .outcomes()
                        .iter()
                        .all(|outcome| outcome.kind == OutcomeKind::Failure
                            && outcome.store.nonce(address(0x101)) == word(0))
                );
            }
        }
    }
}

#[test]
fn same_transaction_selfdestruct_keeps_code_visible_until_final_deletion() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for runtime in [
            hex::decode("60015f55610200ff").unwrap(),
            hex::decode("30ff").unwrap(),
        ] {
            let initcode = init(&runtime, &[]);
            // Both calls use the stored CREATE address. EXTCODESIZE between them
            // must still see runtime bytes; SELFDESTRUCT deletion is end-of-tx.
            let post = hex::decode("805f55505f5f5f5f5f5f5462fffffff1505f543b6001555f543f6002555f5f5f5f5f5f5462fffffff15000").unwrap();
            let bytes = factory(&initcode, true, 7, &post);
            let (world, destination) = fixture(fork, &bytes, &initcode, true);
            let analysis = oracle(world, destination, true, true, true);
            assert!(returned(&analysis).any(|store| {
                store.existence(destination) == Existence::Absent
                    && store.raw_account_code(destination) == Some(Vec::new())
                    && store.read(destination, &word(0), Domain::default()) == word(0)
                    && store
                        .read(address(0x101), &word(1), Domain::default())
                        .contains(U256::from(runtime.len()))
                    && store
                        .read(address(0x101), &word(2), Domain::default())
                        .contains(U256::from_be_slice(keccak256(&runtime).as_slice()))
            }));
        }
    }
}

#[test]
fn initcode_selfdestruct_commits_endowment_then_deletes_the_created_account() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = hex::decode("610200ff").unwrap();
        let bytes = factory(
            &initcode,
            false,
            7,
            &hex::decode("805f55803f6001555000").unwrap(),
        );
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let analysis = oracle(world, destination, true, false, true);
        assert!(returned(&analysis).any(|store| {
            store.existence(destination) == Existence::Absent
                && store.read_balance(address(0x200)) == word(7)
                && store
                    .read(address(0x101), &word(1), Domain::default())
                    .contains(U256::from_be_slice(keccak256([]).as_slice()))
        }));
    }
}

#[test]
fn preexisting_selfdestruct_preserves_code_storage_and_self_beneficiary_balance() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for runtime in [
            hex::decode("610200ff").unwrap(),
            hex::decode("30ff").unwrap(),
        ] {
            let mut world = World::new(fork, "preexisting selfdestruct");
            let mut account = code(&runtime, fork);
            account.balance = word(7);
            account.storage.insert(U256::ZERO, word(42));
            world.insert(address(0x101), account).unwrap();
            world.insert(address(0x200), Account::empty()).unwrap();
            let analysis = oracle_expect_creation(world, address(0x101), false, true, true, false);
            assert_eq!(analysis.status(), Status::Converged);
            assert!(
                returned(&analysis).all(|store| store.raw_account_code(address(0x101))
                    == Some(runtime.clone())
                    && store.existence(address(0x101)) == Existence::Present
                    && store.read(address(0x101), &word(0), Domain::default()) == word(42)
                    && store.read_balance(address(0x101))
                        == word(if runtime == [0x30, 0xff] { 7 } else { 0 }))
            );
        }
    }
}

#[test]
fn reverted_child_selfdestruct_restores_deletion_balance_and_all_deeper_effects() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let runtime = hex::decode("60015f55610200ff").unwrap();
        let initcode = init(&runtime, &[]);
        let post = hex::decode("805f55505f5f5f5f5f61030062fffffff1505f543b60015500").unwrap();
        let bytes = factory(&initcode, true, 7, &post);
        let (mut world, destination) = fixture(fork, &bytes, &initcode, true);
        let mut wrapper = hex::decode("5f5f5f5f5f73").unwrap();
        wrapper.extend(destination.as_slice());
        wrapper.extend(hex::decode("62fffffff1505f5ffd").unwrap());
        world.insert(address(0x300), code(&wrapper, fork)).unwrap();
        let analysis = oracle(world, destination, true, true, true);
        assert!(
            returned(&analysis).any(|store| store.raw_account_code(destination)
                == Some(runtime.clone())
                && store.existence(destination) == Existence::Present
                && store.read_balance(destination) == word(7)
                && store.read_balance(address(0x200)) == word(0)
                && store.read(destination, &word(0), Domain::default()) == word(0))
        );
    }
}

#[test]
fn repeat_create2_collides_even_after_same_transaction_selfdestruct() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let runtime = hex::decode("610200ff").unwrap();
        let initcode = init(&runtime, &[]);
        let mut post = hex::decode("805f55505f5f5f5f5f5f5462fffffff1506005").unwrap();
        push2(&mut post, initcode.len());
        post.extend(hex::decode("5f5ff560015500").unwrap());
        let bytes = factory(&initcode, true, 7, &post);
        let (world, destination) = fixture(fork, &bytes, &initcode, true);
        let analysis = oracle(world, destination, true, true, true);
        assert!(
            returned(&analysis).any(|store| store.nonce(address(0x101)) == word(2)
                && store.read(address(0x101), &word(1), Domain::default()) == word(0)
                && store.existence(destination) == Existence::Absent)
        );
    }
}

#[test]
fn prefunded_creation_resets_previous_persistent_slots_and_retains_balance() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let initcode = init(&[0], &hex::decode("5f54600155").unwrap());
        let bytes = factory(&initcode, false, 7, &return_address());
        let (world, destination) = fixture(fork, &bytes, &initcode, false);
        let mut reset = World::new(fork, "zero nonce and empty code with pre-existing storage");
        for (owner, account) in world.accounts() {
            if *owner != destination {
                reset.insert(*owner, account.clone()).unwrap();
            }
        }
        let mut prefunded = Account::empty();
        prefunded.balance = word(3);
        prefunded.storage.insert(U256::ZERO, word(99));
        reset.insert(destination, prefunded).unwrap();
        let analysis = oracle(reset, destination, true, false, false);
        assert!(
            returned(&analysis).any(|store| store.read_balance(destination) == word(10)
                && store.read(destination, &word(0), Domain::default()) == word(0)
                && store.read(destination, &word(1), Domain::default()) == word(0))
        );
    }
}

#[test]
fn unknown_creation_inputs_report_explicit_frontiers() {
    let initcode = init(&[0], &[]);
    let bytes = factory(&initcode, true, 0, &return_address());
    let (world, destination) = fixture(Fork::Osaka, &bytes, &initcode, true);
    let mut unknown = World::new(Fork::Osaka, "unknown collision");
    for (owner, account) in world.accounts() {
        if *owner != destination {
            unknown.insert(*owner, account.clone()).unwrap();
        }
    }
    let analysis = run(unknown);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.frontiers().iter().any(|frontier| frontier.reason
        == FrontierReason::Creation(CreationBoundary::UnknownCollision(destination))));
    for (program, boundary) in [
        ("5f355f5f5ff5", CreationBoundary::UnknownSalt),
        ("60205f5f3760205f5ff0", CreationBoundary::UnknownInitCode),
    ] {
        let mut world = World::new(Fork::Osaka, "unknown creation input");
        world
            .insert(
                address(0x101),
                Account::from_hex(program, Fork::Osaka).unwrap(),
            )
            .unwrap();
        let analysis = analysis::analyze_world(
            world,
            Entry {
                address: address(0x101),
                caller: address(0x1000),
                value: word(0),
                calldata: ByteArray::unknown(),
                is_static: false,
            },
            ExecutionConfig::default(),
        )
        .unwrap();
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(
            analysis
                .frontiers()
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Creation(boundary.clone()))
        );
    }
}

#[test]
fn store_lifecycle_rollback_and_join_include_code_nonce_and_pending_deletion() {
    let mut world = World::new(Fork::Osaka, "store lifecycle");
    let target = address(0x400);
    world.insert(target, Account::absent()).unwrap();
    world.insert(address(0x200), Account::empty()).unwrap();
    let mut store = Store::new(&world);
    let before = store.clone();
    let saved = store.snapshot();
    store.begin_creation(target);
    store.deploy_code(
        target,
        evm_abstract::bytecode::Program::decode(&[0]).unwrap(),
    );
    store.write(target, &word(0), &word(42), Domain::default());
    store.write_balance(target, word(7));
    store.selfdestruct(target, address(0x200), Domain::default());
    assert_eq!(store.raw_account_code(target), Some(vec![0]));
    assert_eq!(store.pending_destruction(target), Some(true));
    let joined = before.join(&store, Domain::default());
    assert_eq!(joined.created_in_transaction(target), None);
    assert_eq!(joined.pending_destruction(target), None);
    assert_eq!(joined.existence(target), Existence::Unknown);
    assert_eq!(joined.raw_account_code(target), None);
    store.restore(saved);
    assert_eq!(store, before);
}
