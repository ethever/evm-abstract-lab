//! Pinned revm transactions are checked against actual transformed Store bounds.
use super::super::ProcessOwner;
use super::{State, Store, address, catalogue, covers, domain, plan, prepared, word};
use crate::analysis::{ProcessAction, process_creation as creation};
use crate::{
    Fork,
    analysis::OutcomeKind,
    domain::NumericValue,
    resource::WorkBudget,
    world::{Code, EvmAccount, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, U256};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::{TxEnv, result::ExecutionResult},
    database::InMemoryDB,
    interpreter::{
        CreateInputs, CreateOutcome, Interpreter, interpreter::EthInterpreter,
        interpreter_types::Jumps,
    },
    primitives::{Bytes, TxKind, hex},
    state::{AccountInfo, Bytecode, EvmState},
};
#[derive(Default, Debug)]
struct Trace {
    creates: usize,
    init_steps: usize,
    init_returns: usize,
    callback_writes: usize,
    runtime_during_construction: usize,
    destinations: Vec<Address>,
    creators: Vec<Address>,
    constructor_values: Vec<U256>,
    constructor_outcomes: Vec<(OutcomeKind, Option<Address>, Vec<u8>)>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn create(&mut self, _: &mut CTX, input: &mut CreateInputs) -> Option<CreateOutcome> {
        self.creates += 1;
        self.creators.push(input.caller());
        None
    }

    fn create_end(&mut self, _: &mut CTX, _: &CreateInputs, outcome: &mut CreateOutcome) {
        let instruction = *outcome.instruction_result();
        let kind = if instruction.is_ok() {
            OutcomeKind::Return
        } else if instruction.is_revert() {
            OutcomeKind::Revert
        } else {
            OutcomeKind::Failure
        };
        self.constructor_outcomes
            .push((kind, outcome.address, outcome.output().to_vec()));
    }

    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        let input = &interpreter.input;
        if input.bytecode_address.is_none() {
            self.init_steps += 1;
            if interpreter.bytecode.opcode() == 0xf3 {
                self.init_returns += 1;
            }
            assert!(input.input.is_empty(), "constructor calldata must be empty");
            assert!(self.creators.contains(&input.caller_address));
            if !self.destinations.contains(&input.target_address) {
                self.destinations.push(input.target_address);
                self.constructor_values.push(input.call_value);
            }
        } else if input.depth > 0
            && input.target_address == address(0x101)
            && interpreter.bytecode.opcode() == 0x55
        {
            self.callback_writes += 1;
        } else if self.destinations.contains(&input.target_address) {
            self.runtime_during_construction += 1;
        }
    }
}

fn push2(code: &mut Vec<u8>, value: usize) {
    code.extend([0x61, (value >> 8) as u8, value as u8]);
}

fn init(runtime: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut code = prefix.to_vec();
    let offset = code.len() + 13;
    push2(&mut code, runtime.len());
    push2(&mut code, offset);
    code.extend([0x5f, 0x39]);
    push2(&mut code, runtime.len());
    code.extend([0x5f, 0xf3]);
    assert_eq!(code.len(), offset);
    code.extend(runtime);
    code
}

fn factory(initcode: &[u8], salt: Option<U256>, prefix: &[u8], post: &[u8]) -> Vec<u8> {
    let mut code = prefix.to_vec();
    push2(&mut code, initcode.len());
    let offset_patch = code.len() + 1;
    push2(&mut code, 0);
    code.extend([0x5f, 0x39]);
    if let Some(salt) = salt {
        code.push(0x7f);
        code.extend(salt.to_be_bytes::<32>());
    }
    push2(&mut code, initcode.len());
    code.extend([0x5f, 0x60, 7, if salt.is_some() { 0xf5 } else { 0xf0 }]);
    code.extend(post);
    let offset = code.len();
    code[offset_patch] = (offset >> 8) as u8;
    code[offset_patch + 1] = offset as u8;
    code.extend(initcode);
    code
}

fn execute(
    fork: Fork,
    root: Vec<u8>,
    extra: &[(Address, Vec<u8>, u64, U256)],
    nonce: u64,
) -> (ExecutionResult, EvmState, Trace) {
    let mut db = InMemoryDB::default();
    for (owner, bytes, nonce, balance) in
        std::iter::once((address(0x101), root, nonce, U256::from(100))).chain(
            extra
                .iter()
                .map(|(owner, code, nonce, balance)| (*owner, code.clone(), *nonce, *balance)),
        )
    {
        let code = Bytecode::new_raw(Bytes::from(bytes));
        db.insert_account_info(
            owner,
            AccountInfo::new(balance, nonce, code.hash_slow(), code),
        );
    }
    db.insert_account_info(
        address(0x1000),
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
                .caller(address(0x1000))
                .kind(TxKind::Call(address(0x101)))
                .gas_limit(1_000_000)
                .build()
                .unwrap(),
        )
        .unwrap();
    (result.result, result.state, evm.inspector)
}

fn matching_world(
    fork: Fork,
    root: &[u8],
    destination: Address,
    protected: bool,
) -> (EvmWorld<NumericValue>, Store) {
    let mut world = EvmWorld::new(fork, "allocation/revm matched world");
    let mut root = EvmAccount::from_hex(&hex::encode(root), fork).unwrap();
    root.balance = word(100);
    root.nonce = word(7);
    world.insert(address(0x101), root).unwrap();
    let mut dest = if protected {
        EvmAccount::from_hex("00", fork).unwrap()
    } else {
        EvmAccount::empty()
    };
    dest.balance = word(23);
    world.insert(destination, dest).unwrap();
    world.insert(address(3), EvmAccount::empty()).unwrap();
    world.insert(address(0x404), EvmAccount::empty()).unwrap();
    let d = domain();
    let store = EvmStore::new(&world, d.borrowed());
    (world, store)
}
fn assert_native_bounds(upper: &State, actual: &EvmState) {
    let d = domain();
    let mut budget = WorkBudget::new(100_000_000);
    let replay = upper
        .instantiate_with_payment(d.borrowed(), &mut budget)
        .unwrap();
    for (owner, account) in actual {
        // revm returns a journal patch: DatabaseCommit deletes this entire
        // account even though AccountInfo can retain nonce=1. Compare against
        // the committed state after native transaction finalization.
        if account.is_selfdestructed() {
            assert_eq!(replay.existence(*owner), crate::world::Existence::Absent);
            assert_eq!(replay.nonce(*owner).singleton(), Some(U256::ZERO));
            assert_eq!(replay.read_balance(*owner).singleton(), Some(U256::ZERO));
            assert!(matches!(replay.code(*owner), Some(Code::Empty)));
            for slot in account.storage.keys() {
                assert_eq!(
                    replay
                        .read(
                            *owner,
                            &crate::domain::EvmValue::constant(*slot),
                            d.borrowed()
                        )
                        .singleton(),
                    Some(U256::ZERO)
                );
            }
            continue;
        }
        assert!(
            replay
                .read_balance(*owner)
                .numeric()
                .contains(account.info.balance),
            "balance {owner}"
        );
        assert!(
            replay
                .nonce(*owner)
                .numeric()
                .contains(U256::from(account.info.nonce)),
            "nonce {owner}"
        );
        for (slot, value) in &account.storage {
            assert!(
                replay
                    .read(
                        *owner,
                        &crate::domain::EvmValue::constant(*slot),
                        d.borrowed()
                    )
                    .numeric()
                    .contains(value.present_value()),
                "slot {owner}:{slot}"
            );
        }
        if let Some(code) = account.info.code.as_ref() {
            match replay.code(*owner) {
                None | Some(Code::Unknown) => {}
                Some(Code::Empty) => assert!(code.original_bytes().is_empty(), "code {owner}"),
                Some(Code::Runtime(program)) => assert_eq!(
                    program.bytes(),
                    code.original_bytes().as_ref(),
                    "code {owner}"
                ),
                Some(Code::Delegation(_)) => panic!("no delegation fixture"),
            }
        }
    }
}

#[test]
fn joint_revm_success_named_and_residual_prefund_empty_runtime_lifecycle() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for salt in [None, Some(U256::from(19))] {
            // Self CALL actually enters an Empty account, never reexecutes initcode.
            let prefix = hex::decode("5f5f5f5f5f3062fffffff150").unwrap();
            let initcode = init(&[0x60, 0x42, 0], &prefix);
            let creator = address(0x101);
            let destination = salt.map_or_else(
                || creator.create(7),
                |salt| creator.create2_from_code(salt.to_be_bytes::<32>(), &initcode),
            );
            let root = factory(
                &initcode,
                salt,
                &[],
                &hex::decode("5f523d60205260405ff3").unwrap(),
            );
            let extra = [(destination, vec![], 0, U256::from(23))];
            let (result, state, trace) = execute(fork, root.clone(), &extra, 7);
            assert!(result.is_success(), "{fork} {trace:?} {result:?}");
            assert_eq!(trace.creates, 1);
            assert!(trace.init_steps > 0);
            assert_eq!(trace.runtime_during_construction, 0);
            assert_eq!(trace.constructor_values, [U256::from(7)]);
            assert_eq!(trace.constructor_outcomes[0].0, OutcomeKind::Return);
            assert_eq!(state[&destination].info.balance, U256::from(30));
            assert_eq!(state[&destination].info.nonce, 1);
            assert_eq!(state[&creator].info.nonce, 8);
            assert_eq!(
                U256::from_be_slice(&result.output().unwrap()[..32]),
                U256::from_be_slice(destination.as_slice())
            );
            assert_eq!(
                U256::from_be_slice(&result.output().unwrap()[32..]),
                U256::ZERO
            );
            for named in [true, false] {
                let (_, native) = matching_world(fork, &root, destination, false);
                let names = if named {
                    catalogue(&[creator, destination, address(3), address(0x404)])
                } else {
                    catalogue(&[creator, address(3), address(0x404)])
                };
                let d = domain();
                let mut budget = WorkBudget::new(100_000_000);
                let input = prepared(&native, &names, &d, &mut budget);
                let p = plan(
                    &input,
                    &names,
                    ProcessOwner::Named(creator),
                    if salt.is_some() {
                        ProcessAction::Create2
                    } else {
                        ProcessAction::Create
                    },
                    &mut budget,
                );
                let class = names.classify(destination);
                assert!(
                    p.constructors
                        .iter()
                        .any(|entry| entry.destination == class)
                );
                let post = input
                    .process_increment_nonce(
                        ProcessOwner::Named(creator),
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap();
                let child = post
                    .process_begin_constructor(
                        ProcessOwner::Named(creator),
                        class,
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap();
                let bound = child
                    .process_deploy(class, d.borrowed(), &mut budget)
                    .unwrap();
                assert_native_bounds(&bound, &state);
                println!(
                    "allocation-revm-success {}",
                    serde_json::json!({"fork":fork.to_string(),"salted":salt.is_some(),"named":named,"destination":destination.to_string(),"epoch":names.epoch().to_string(),"work":budget.used()})
                );
            }
        }
    }
}

#[test]
fn joint_revm_child_revert_invalid_deposit_oog_and_collision_keep_post_nonce() {
    let cases = [
        ("revert", hex::decode("602a5f555f5ffd").unwrap(), false),
        ("invalid-init", vec![0xfe], false),
        (
            "ripemd-revert",
            hex::decode("5f5f5f5f5f600362fffffff1505f5ffd").unwrap(),
            false,
        ),
        ("invalid-runtime", init(&[0xef], &[]), false),
        ("oversized-runtime", init(&vec![0; 24_577], &[]), false),
        ("deposit-oog", init(&vec![0; 20_000], &[]), false),
        (
            "constructor-oog",
            hex::decode("5b5f50600056").unwrap(),
            false,
        ),
        ("occupied-code", init(&[0], &[]), true),
    ];
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for salt in [None, Some(U256::from(23))] {
            for (label, initcode, occupied) in &cases {
                let creator = address(0x101);
                let destination = salt.map_or_else(
                    || creator.create(7),
                    |salt| creator.create2_from_code(salt.to_be_bytes::<32>(), initcode),
                );
                let root = factory(
                    initcode,
                    salt,
                    &[],
                    &hex::decode("5f523d60205260405ff3").unwrap(),
                );
                let (result, state, trace) = execute(
                    fork,
                    root.clone(),
                    &[(
                        destination,
                        if *occupied { vec![0] } else { vec![] },
                        0,
                        U256::from(23),
                    )],
                    7,
                );
                assert!(result.is_success(), "{label} {fork} {result:?}");
                assert_eq!(
                    U256::from_be_slice(&result.output().unwrap()[..32]),
                    U256::ZERO,
                    "{label}"
                );
                assert_eq!(state[&creator].info.nonce, 8, "{label}");
                assert_eq!(state[&creator].info.balance, U256::from(100), "{label}");
                assert_eq!(state[&destination].info.nonce, 0, "{label}");
                assert_eq!(state[&destination].info.balance, U256::from(23), "{label}");
                let (_, native) = matching_world(fork, &root, destination, *occupied);
                let d = domain();
                let names = catalogue(&[creator, destination, address(3), address(0x404)]);
                let mut budget = WorkBudget::new(100_000_000);
                let input = prepared(&native, &names, &d, &mut budget);
                let p = plan(
                    &input,
                    &names,
                    ProcessOwner::Named(creator),
                    if salt.is_some() {
                        ProcessAction::Create2
                    } else {
                        ProcessAction::Create
                    },
                    &mut budget,
                );
                let post = input
                    .process_increment_nonce(
                        ProcessOwner::Named(creator),
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap();
                if *occupied {
                    assert!(
                        !p.constructors
                            .iter()
                            .any(|e| e.destination == ProcessOwner::Named(destination))
                    );
                    assert_eq!(trace.init_steps, 0);
                } else {
                    let entry = *p
                        .constructors
                        .iter()
                        .find(|e| e.destination == ProcessOwner::Named(destination))
                        .unwrap();
                    // Deployment validation occurs AFTER initcode RETURN. Test
                    // Return's failure alternative explicitly, not by conflating
                    // create_end's already-validated failure with initcode exit.
                    let exit = if matches!(
                        *label,
                        "invalid-runtime" | "oversized-runtime" | "deposit-oog"
                    ) {
                        assert_eq!(
                            trace.init_returns, 1,
                            "observed initcode RETURN before deployment validation: {label}"
                        );
                        assert_eq!(trace.constructor_outcomes[0].0, OutcomeKind::Failure);
                        OutcomeKind::Return
                    } else {
                        trace.constructor_outcomes[0].0
                    };
                    let cases = creation::complete(entry, exit, &mut budget).unwrap();
                    assert!(
                        cases.into_iter().flatten().any(|case| case.store
                            == creation::StoreDisposition::RestorePostNonceCheckpoint)
                    );
                }
                let failed_state = if *occupied {
                    post.clone()
                } else {
                    post.process_begin_constructor(
                        ProcessOwner::Named(creator),
                        ProcessOwner::Named(destination),
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap()
                };
                let mut failed = failed_state
                    .instantiate_with_payment(d.borrowed(), &mut budget)
                    .unwrap();
                let saved = crate::world::EvmSnapshot::from_state(
                    post.instantiate_with_payment(d.borrowed(), &mut budget)
                        .unwrap(),
                );
                failed.write(destination, &word(0), &word(42), d.borrowed());
                if *label == "ripemd-revert" {
                    failed.touch_account(address(3));
                }
                failed.restore(saved);
                assert_eq!(
                    failed.touched_in_transaction(address(3)),
                    Some(*label == "ripemd-revert")
                );
                if *label == "ripemd-revert" {
                    assert!(state[&address(3)].is_touched());
                }
                assert_native_bounds(
                    &failed
                        .summary_projection_with_payment(d.borrowed(), &mut budget)
                        .unwrap(),
                    &state,
                );
                println!(
                    "allocation-revm-failure {}",
                    serde_json::json!({"fork":fork.to_string(),"salted":salt.is_some(),"case":label,"observed_initcode_returns":trace.init_returns,"validated_exit":format!("{:?}",trace.constructor_outcomes.first().map(|v|v.0)),"work":budget.used()})
                );
            }
        }
    }
}

#[test]
fn joint_selfdestruct_child_commit_then_outer_rollback_preserves_only_actual_ripemd_touch() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for beneficiary in [address(3), address(0x404)] {
            for outer_revert in [false, true] {
                let mut initcode = vec![0x73];
                initcode.extend(beneficiary.as_slice());
                initcode.push(0xff);
                let creator = address(0x101);
                let destination = creator.create(7);
                let post = if outer_revert {
                    hex::decode("505f5ffd").unwrap()
                } else {
                    hex::decode("5f523d60205260405ff3").unwrap()
                };
                let root = factory(&initcode, None, &[], &post);
                let extras = [
                    (destination, vec![], 0, U256::from(23)),
                    (beneficiary, vec![], 0, U256::ZERO),
                ];
                let (result, state, trace) = execute(fork, root.clone(), &extras, 7);
                assert_eq!(
                    matches!(result, ExecutionResult::Revert { .. }),
                    outer_revert
                );
                assert_eq!(trace.constructor_outcomes[0].0, OutcomeKind::Return);
                let (_, mut native) = matching_world(fork, &root, destination, false);
                let d = domain();
                let names = catalogue(&[creator, destination, address(3), address(0x404)]);
                let mut budget = WorkBudget::new(100_000_000);
                let outer = native.snapshot();
                let before = prepared(&native, &names, &d, &mut budget);
                assert_eq!(
                    before
                        .touched
                        .get(&address(3))
                        .copied()
                        .unwrap_or(before.touched_default),
                    Some(false)
                );
                let p = plan(
                    &before,
                    &names,
                    ProcessOwner::Named(creator),
                    ProcessAction::Create,
                    &mut budget,
                );
                let entry = *p
                    .constructors
                    .iter()
                    .find(|e| e.destination == ProcessOwner::Named(destination))
                    .unwrap();
                let post_nonce = before
                    .process_increment_nonce(
                        ProcessOwner::Named(creator),
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap();
                let child = post_nonce
                    .process_begin_constructor(
                        ProcessOwner::Named(creator),
                        ProcessOwner::Named(destination),
                        d.borrowed(),
                        &mut budget,
                    )
                    .unwrap();
                native.write_nonce(creator, word(8));
                native.write_balance(creator, word(93));
                native.write_balance(destination, word(30));
                native.begin_creation(destination);
                assert!(covers(&child, &native, &names, &d));
                native.selfdestruct(destination, beneficiary, d.borrowed());
                assert_eq!(native.pending_destruction(destination), Some(true));
                let effects = prepared(&native, &names, &d, &mut budget);
                let deployed = effects
                    .process_deploy(ProcessOwner::Named(destination), d.borrowed(), &mut budget)
                    .unwrap();
                assert_eq!(
                    deployed.pending_destruction.get(&destination),
                    Some(&Some(true))
                );
                assert_eq!(deployed.created.get(&destination), Some(&Some(true)));
                assert_eq!(
                    creation::complete(entry, OutcomeKind::Return, &mut budget).unwrap()[0]
                        .unwrap()
                        .store,
                    creation::StoreDisposition::CommitConstructor
                );
                // Check touch at the active-transaction/rollback boundary.
                // Finalization deliberately clears native transaction flags;
                // revm's returned EvmState still carries its journal statuses.
                assert_eq!(
                    native.touched_in_transaction(address(3)),
                    Some(beneficiary == address(3))
                );
                if outer_revert {
                    native.restore(outer);
                    assert_eq!(
                        native.touched_in_transaction(address(3)),
                        Some(beneficiary == address(3))
                    );
                } else {
                    native.finalize_transaction(d.borrowed()).unwrap();
                    assert_eq!(native.touched_in_transaction(address(3)), Some(false));
                }
                if outer_revert {
                    assert_eq!(native.nonce(creator).singleton(), Some(U256::from(7)));
                    assert_eq!(
                        native.read_balance(destination).singleton(),
                        Some(U256::from(23))
                    );
                    assert_eq!(native.created_in_transaction(destination), Some(false));
                }
                let final_bound = prepared(&native, &names, &d, &mut budget);
                assert_native_bounds(&final_bound, &state);
                if beneficiary == address(3) {
                    assert!(state[&beneficiary].is_touched());
                }
                println!(
                    "allocation-selfdestruct {}",
                    serde_json::json!({"fork":fork.to_string(),"ripemd":beneficiary==address(3),"outer_revert":outer_revert,"work":budget.used()})
                );
            }
        }
    }
}

#[test]
fn joint_callback_deployment_commits_before_the_exact_enclosing_rollback() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for salt in [None, Some(U256::from(19))] {
            let creator = address(0x101);
            let callback = hex::decode("60015f525f5f60205f5f61010162fffffff150").unwrap();
            let initcode = init(&[0], &callback);
            let destination = salt.map_or_else(
                || creator.create(7),
                |salt| creator.create2_from_code(salt.to_be_bytes::<32>(), &initcode),
            );
            let root = factory(
                &initcode,
                salt,
                &hex::decode("5f351561000c57602a5f55005b").unwrap(),
                &hex::decode("505f5ffd").unwrap(),
            );
            let (result, state, trace) = execute(
                fork,
                root.clone(),
                &[(destination, vec![], 0, U256::from(23))],
                7,
            );
            assert!(matches!(result, ExecutionResult::Revert { .. }));
            assert!(trace.callback_writes > 0);
            assert_eq!(trace.constructor_outcomes[0].0, OutcomeKind::Return);
            let (_, mut native) = matching_world(fork, &root, destination, false);
            let original = native.clone();
            let outer = native.snapshot();
            let d = domain();
            let names = catalogue(&[creator, destination, address(3), address(0x404)]);
            let mut budget = WorkBudget::new(100_000_000);
            let before = prepared(&native, &names, &d, &mut budget);
            let p = plan(
                &before,
                &names,
                ProcessOwner::Named(creator),
                if salt.is_some() {
                    ProcessAction::Create2
                } else {
                    ProcessAction::Create
                },
                &mut budget,
            );
            let entry = *p
                .constructors
                .iter()
                .find(|e| e.destination == ProcessOwner::Named(destination))
                .unwrap();
            let post = before
                .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
                .unwrap();
            let child = post
                .process_begin_constructor(
                    ProcessOwner::Named(creator),
                    ProcessOwner::Named(destination),
                    d.borrowed(),
                    &mut budget,
                )
                .unwrap();
            let callback_bound = child
                .process_havoc_storage(creator, d.borrowed(), &mut budget)
                .unwrap();
            let committed = callback_bound
                .process_deploy(ProcessOwner::Named(destination), d.borrowed(), &mut budget)
                .unwrap();
            native.write_nonce(creator, word(8));
            native.write_balance(creator, word(93));
            native.write_balance(destination, word(30));
            native.begin_creation(destination);
            native.write(creator, &word(0), &word(42), d.borrowed());
            native.deploy_code(
                destination,
                crate::bytecode::Program::from_hex("00").unwrap(),
            );
            // The havoced callback bound has independent current storage,
            // whereas precise SSTORE retains acquisition bookkeeping on other
            // slots. Check the complete changed data witness independently of
            // that analysis-only dependency order.
            let delivered = committed
                .instantiate_with_payment(d.borrowed(), &mut budget)
                .unwrap();
            assert!(
                delivered
                    .read(creator, &word(0), d.borrowed())
                    .numeric()
                    .contains(U256::from(42))
            );
            assert!(
                delivered
                    .read_balance(creator)
                    .numeric()
                    .contains(U256::from(93))
            );
            assert!(
                delivered
                    .read_balance(destination)
                    .numeric()
                    .contains(U256::from(30))
            );
            assert_eq!(delivered.nonce(creator).singleton(), Some(U256::from(8)));
            assert_eq!(delivered.nonce(destination).singleton(), Some(U256::ONE));
            assert_eq!(delivered.created_in_transaction(destination), Some(true));
            assert_eq!(delivered.pending_destruction(destination), Some(false));
            assert_eq!(delivered.code(creator), native.code(creator));
            assert!(matches!(
                delivered.code(destination),
                None | Some(Code::Unknown)
            ));
            assert_eq!(
                native.read(creator, &word(0), d.borrowed()).singleton(),
                Some(U256::from(42))
            );
            assert_eq!(native.created_in_transaction(destination), Some(true));
            assert_eq!(
                creation::complete(entry, OutcomeKind::Return, &mut budget).unwrap()[0]
                    .unwrap()
                    .store,
                creation::StoreDisposition::CommitConstructor
            );
            native.restore(outer);
            assert_eq!(native, original);
            assert_native_bounds(&prepared(&native, &names, &d, &mut budget), &state);
            println!(
                "allocation-callback {}",
                serde_json::json!({"fork":fork.to_string(),"salted":salt.is_some(),"callback_writes":trace.callback_writes,"work":budget.used()})
            );
        }
    }
}
