//! Independent pinned revm witnesses; no alternate interpreter or scheduler.

use alloy_primitives::{Address, U256, keccak256};
use evm_abstract::Fork;
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

use crate::{
    analysis::{OutcomeKind, ProcessAction, ProcessOwner},
    creation::{
        Collision, CompletionReason, ConstructorCode, CreationResult, Destination, ReturnedData,
        StoreDisposition, complete, prepare,
    },
    resource::WorkBudget,
    rules::{address, request, targets},
};

#[derive(Default, Debug)]
struct Trace {
    creates: usize,
    init_steps: usize,
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
    extra: &[(Address, Vec<u8>, u64)],
    nonce: u64,
) -> (ExecutionResult, EvmState, Trace) {
    let mut db = InMemoryDB::default();
    for (owner, bytes, nonce, balance) in
        std::iter::once((address(0x101), root, nonce, U256::from(100))).chain(
            extra
                .iter()
                .map(|(owner, code, nonce)| (*owner, code.clone(), *nonce, U256::ZERO)),
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
                .gas_limit(10_000_000)
                .build()
                .unwrap(),
        )
        .unwrap();
    (result.result, result.state, evm.inspector)
}

const RETURN_ADDRESS_AND_SIZE: &str = "5f523d60205260405ff3";

#[test]
fn finite_native_creation_covers_arbitrary_salt_initcode_runtime_and_real_address() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for (salt, nonce) in [
            (None, 0),
            (None, 2),
            (Some(U256::ZERO), 0),
            (Some(U256::from(5)), 2),
            (Some(U256::MAX), 0),
        ] {
            let action = if salt.is_some() {
                ProcessAction::Create2
            } else {
                ProcessAction::Create
            };
            let prefix =
                hex::decode("335f553460015536600255303b6003555f5f5f5f5f3062fffffff1600455")
                    .unwrap();
            let runtime = if nonce == 2 {
                vec![0x60, 0x42, 0x00]
            } else {
                vec![0]
            };
            let initcode = init(&runtime, &prefix);
            let destination = salt.map_or_else(
                || address(0x101).create(nonce),
                |salt| address(0x101).create2_from_code(salt.to_be_bytes::<32>(), &initcode),
            );
            let bytes = factory(
                &initcode,
                salt,
                &[],
                &hex::decode(RETURN_ADDRESS_AND_SIZE).unwrap(),
            );
            let (result, state, trace) = execute(fork, bytes, &[], nonce);
            assert!(result.is_success(), "{fork} {trace:?} {result:?}");
            let output = result.output().unwrap();
            assert_eq!(output.len(), 64);
            let returned_address = U256::from_be_slice(&output[..32]);
            assert_eq!(
                returned_address,
                U256::from_be_slice(destination.as_slice())
            );
            assert_ne!(
                returned_address,
                U256::from(1),
                "CREATE must not be a CALL boolean"
            );
            assert_ne!(returned_address, U256::ZERO);
            assert_eq!(U256::from_be_slice(&output[32..]), U256::ZERO);
            assert_eq!(
                trace.creates, 1,
                "CALL to under-construction account must not execute initcode"
            );
            assert!(trace.init_steps > 0);
            assert_eq!(trace.runtime_during_construction, 0);
            assert_eq!(trace.creators, [address(0x101)]);
            assert_eq!(trace.constructor_values, [U256::from(7)]);
            let deployed = &state[&destination];
            assert_eq!(
                deployed
                    .info
                    .code
                    .as_ref()
                    .unwrap()
                    .original_bytes()
                    .as_ref(),
                runtime.as_slice()
            );
            assert_eq!(deployed.info.balance, U256::from(7));
            assert_eq!(deployed.info.nonce, 1);
            for (slot, expected) in [
                (0, U256::from(0x101)),
                (1, U256::from(7)),
                (2, U256::ZERO),
                (3, U256::ZERO),
                (4, U256::from(1)),
            ] {
                assert_eq!(
                    deployed
                        .storage
                        .get(&U256::from(slot))
                        .map_or(U256::ZERO, |value| value.present_value()),
                    expected
                );
            }
            assert_eq!(state[&address(0x101)].info.nonce, nonce + 1);
            assert_eq!(state[&address(0x101)].info.balance, U256::from(93));
            let mut budget = WorkBudget::new(100_000);
            let plan = prepare(request(action), &targets(), 17, &mut budget).unwrap();
            // All these hash-derived destinations are outside the retained names.
            assert!(
                !targets()
                    .iter()
                    .any(|cell| cell.owner == ProcessOwner::Named(destination))
            );
            let entry = *plan
                .constructors
                .iter()
                .find(|entry| entry.destination == ProcessOwner::Residual)
                .unwrap();
            let success = complete(entry, OutcomeKind::Return, &mut budget).unwrap()[0].unwrap();
            assert_eq!(
                success.result,
                CreationResult::Address(ProcessOwner::Residual)
            );
            assert_eq!(success.returndata, ReturnedData::Empty);
            assert!(runtime.len() <= success.deployment.unwrap().maximum_runtime_size);
            assert_ne!(
                runtime.first(),
                Some(&success.deployment.unwrap().forbidden_first_byte)
            );
            println!(
                "creation-witness {}",
                serde_json::json!({"fork":fork.to_string(), "action":format!("{action:?}"),
                "destination":destination.to_string(), "nonce":nonce, "init_steps":trace.init_steps, "work":budget.used()})
            );
        }
    }
}

#[test]
fn constructor_callback_then_outer_revert_uses_both_rollback_scopes() {
    for salt in [None, Some(U256::from(19))] {
        let callback = hex::decode("60015f525f5f60205f5f61010162fffffff150").unwrap();
        let initcode = init(&[0], &callback);
        let dispatch = hex::decode("5f351561000c57602a5f55005b").unwrap();
        let bytes = factory(
            &initcode,
            salt,
            &dispatch,
            &hex::decode("505f5ffd").unwrap(),
        );
        let (result, state, trace) = execute(Fork::Osaka, bytes, &[], 0);
        assert!(
            matches!(result, ExecutionResult::Revert { .. }),
            "{trace:?} {result:?}"
        );
        assert_eq!(trace.creates, 1);
        assert!(
            trace.callback_writes > 0,
            "constructor must really call back and SSTORE"
        );
        assert_eq!(trace.constructor_outcomes[0].0, OutcomeKind::Return);
        let owner = &state[&address(0x101)];
        assert_eq!(owner.info.nonce, 0);
        assert_eq!(owner.info.balance, U256::from(100));
        assert_eq!(
            owner
                .storage
                .get(&U256::ZERO)
                .map_or(U256::ZERO, |value| value.present_value()),
            U256::ZERO
        );
        for destination in &trace.destinations {
            assert!(state.get(destination).is_none_or(|account| {
                account.info.nonce == 0
                    && account.info.balance.is_zero()
                    && account
                        .info
                        .code
                        .as_ref()
                        .is_none_or(|code| code.original_bytes().is_empty())
            }));
        }
        let mut budget = WorkBudget::new(100_000);
        let action = if salt.is_some() {
            ProcessAction::Create2
        } else {
            ProcessAction::Create
        };
        let plan = prepare(request(action), &targets(), 17, &mut budget).unwrap();
        let entry = *plan.constructors.last().unwrap();
        assert!(entry.actions.contains(&ProcessAction::Call));
        assert!(entry.actions.contains(&action));
        assert_eq!(
            complete(entry, OutcomeKind::Revert, &mut budget).unwrap()[0]
                .unwrap()
                .store,
            StoreDisposition::RestorePostNonceCheckpoint
        );
        println!(
            "creation-rollback action={action:?} callback_writes={} root_nonce={} root_balance={}",
            trace.callback_writes, owner.info.nonce, owner.info.balance
        );
    }
}

#[test]
fn collision_constructor_revert_and_exception_have_zero_result_and_correct_data() {
    let mut budget = WorkBudget::new(100_000);
    for salt in [None, Some(U256::from(5))] {
        let action = if salt.is_some() {
            ProcessAction::Create2
        } else {
            ProcessAction::Create
        };
        for (initcode, collision, expected_kind, expected_size) in [
            (init(&[0], &[]), true, OutcomeKind::Failure, 0),
            (
                hex::decode("60075f5360015ffd").unwrap(),
                false,
                OutcomeKind::Revert,
                1,
            ),
            (vec![0xfe], false, OutcomeKind::Failure, 0),
            (init(&[0xef], &[]), false, OutcomeKind::Failure, 0),
            (init(&vec![0; 24_577], &[]), false, OutcomeKind::Failure, 0),
            (
                hex::decode("5b5f56").unwrap(),
                false,
                OutcomeKind::Failure,
                0,
            ),
        ] {
            let destination = salt.map_or_else(
                || address(0x101).create(0),
                |salt| address(0x101).create2_from_code(salt.to_be_bytes::<32>(), &initcode),
            );
            let extra = if collision {
                vec![(destination, vec![0], 0)]
            } else {
                vec![]
            };
            let bytes = factory(
                &initcode,
                salt,
                &[],
                &hex::decode(RETURN_ADDRESS_AND_SIZE).unwrap(),
            );
            let (result, state, trace) = execute(Fork::Osaka, bytes, &extra, 0);
            assert!(result.is_success(), "{result:?} {trace:?}");
            let output = result.output().unwrap();
            assert_eq!(U256::from_be_slice(&output[..32]), U256::ZERO);
            assert_eq!(
                U256::from_be_slice(&output[32..]),
                U256::from(expected_size)
            );
            assert_eq!(state[&address(0x101)].info.nonce, 1);
            assert_eq!(state[&address(0x101)].info.balance, U256::from(100));
            assert_eq!(trace.constructor_outcomes[0].0, expected_kind);
            let cells = [
                Destination {
                    owner: ProcessOwner::Named(destination),
                    collision: if collision {
                        Collision::NonEmptyCode
                    } else {
                        Collision::Available
                    },
                },
                Destination {
                    owner: ProcessOwner::Residual,
                    collision: Collision::Unknown,
                },
            ];
            let mut input = request(action);
            input.code = ConstructorCode::CapturedInitCode(keccak256(&initcode));
            let plan = prepare(input, &cells, 17, &mut budget).unwrap();
            if collision {
                assert_eq!(trace.init_steps, 0);
                assert!(
                    !plan
                        .constructors
                        .iter()
                        .any(|entry| entry.destination == ProcessOwner::Named(destination))
                );
                assert_eq!(
                    state[&destination]
                        .info
                        .code
                        .as_ref()
                        .unwrap()
                        .original_bytes()
                        .as_ref(),
                    &[0]
                );
            } else {
                let entry = *plan
                    .constructors
                    .iter()
                    .find(|entry| entry.destination == ProcessOwner::Named(destination))
                    .unwrap();
                let completion = complete(entry, expected_kind, &mut budget).unwrap()[0].unwrap();
                assert_eq!(completion.result, CreationResult::Zero);
                assert_eq!(
                    completion.returndata,
                    if expected_size == 0 {
                        ReturnedData::Empty
                    } else {
                        ReturnedData::ConstructorRevert
                    }
                );
                assert_ne!(completion.reason, CompletionReason::Deployed);
                assert_eq!(
                    completion.store,
                    StoreDisposition::RestorePostNonceCheckpoint
                );
            }
        }
    }
}

#[test]
fn static_exception_and_delegate_callcode_creator_use_actual_execution_owner() {
    let initcode = init(&[0], &[]);
    let library = factory(
        &initcode,
        None,
        &[],
        &hex::decode(RETURN_ADDRESS_AND_SIZE).unwrap(),
    );
    for prefix in ["5f5f5f5f61020262fffffff4", "5f5f5f5f5f61020262fffffff2"] {
        let root = hex::decode(format!("{prefix}503d5f5f3e3d5ff3")).unwrap();
        let (result, state, trace) = execute(
            Fork::Osaka,
            root,
            &[(address(0x202), library.clone(), 0)],
            0,
        );
        assert!(result.is_success(), "{result:?} {trace:?}");
        assert_eq!(trace.creators, [address(0x101)]);
        assert_eq!(trace.destinations, [address(0x101).create(0)]);
        assert_eq!(trace.constructor_values, [U256::from(7)]);
        assert_eq!(state[&address(0x101)].info.nonce, 1);
        assert_eq!(state[&address(0x202)].info.nonce, 0);
    }
    let root = hex::decode("5f5f5f5f61020262fffffffa5f5260205ff3").unwrap();
    let (result, state, trace) = execute(Fork::Osaka, root, &[(address(0x202), library, 0)], 0);
    assert!(result.is_success());
    assert_eq!(U256::from_be_slice(result.output().unwrap()), U256::ZERO);
    assert_eq!(trace.creates, 0);
    assert_eq!(trace.init_steps, 0);
    assert_eq!(state[&address(0x202)].info.nonce, 0);
}

#[test]
fn native_constructor_creation_and_max_nonce_rejection_are_distinct_rules() {
    let nested = factory(
        &init(&[0], &[]),
        None,
        &[],
        &hex::decode("505f5ff3").unwrap(),
    );
    let root = factory(
        &nested,
        None,
        &[],
        &hex::decode(RETURN_ADDRESS_AND_SIZE).unwrap(),
    );
    let (result, state, trace) = execute(Fork::Osaka, root, &[], 0);
    assert!(result.is_success(), "{result:?} {trace:?}");
    let first = address(0x101).create(0);
    let second = first.create(1);
    assert_eq!(trace.creates, 2);
    assert_eq!(trace.creators, [address(0x101), first]);
    assert_eq!(trace.destinations, [first, second]);
    assert_eq!(state[&first].info.nonce, 2);
    assert_eq!(state[&first].info.balance, U256::ZERO);
    assert_eq!(state[&second].info.balance, U256::from(7));
    let mut input = request(ProcessAction::Create);
    input.creator = ProcessOwner::Residual;
    let mut budget = WorkBudget::new(100_000);
    let plan = prepare(input, &targets(), 17, &mut budget).unwrap();
    let entry = plan.constructors.last().unwrap();
    assert_eq!(entry.creator, ProcessOwner::Residual);
    assert_eq!(entry.destination, ProcessOwner::Residual);
    assert_ne!(
        first, second,
        "the two residual instances must remain distinguishable in concrete refinements"
    );
    assert!(entry.actions.contains(&ProcessAction::Create));

    let root = factory(
        &init(&[0], &[]),
        None,
        &[],
        &hex::decode(RETURN_ADDRESS_AND_SIZE).unwrap(),
    );
    let (result, state, trace) = execute(Fork::Osaka, root, &[], u64::MAX);
    assert!(result.is_success(), "{result:?} {trace:?}");
    assert_eq!(
        U256::from_be_slice(&result.output().unwrap()[..32]),
        U256::ZERO
    );
    assert_eq!(trace.init_steps, 0);
    assert_eq!(state[&address(0x101)].info.nonce, u64::MAX);
}
