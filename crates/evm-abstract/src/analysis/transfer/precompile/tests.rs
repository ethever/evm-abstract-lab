//! Finite provider comparisons and production transfer/continuation regressions.
//! Byte/length inclusion checks are not a proof of the complete EVM engine.

use super::*;
use crate::{
    analysis::{
        self, ExecutionConfig, FrameCode, MachineEdgeKind, MachinePayload, Status,
        control::Control,
        machine::fixtures,
        transfer::{StorageReadPolicy, WorkBudget, empty_execution, initial},
    },
    domain::relational::QueryReason,
    world::{Account, Entry, World},
};
use revm_precompile::{PrecompileError, PrecompileHalt, PrecompileOutput};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn value(number: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn config() -> ExecutionConfig {
    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    config
}

fn fixture(
    target: Address,
    input: ByteArray,
    fork: Fork,
    config: &ExecutionConfig,
) -> (World, Entry, MachinePayload) {
    let world = World::new(fork, "pure precompile fallback");
    let entry = Entry::concrete(target, address(0x900), value(0), input);
    let payload = initial(&world, &entry, config.domain().unwrap()).unwrap();
    (world, entry, payload)
}

fn run(
    world: &World,
    entry: &Entry,
    payload: MachinePayload,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
) -> Execution {
    super::super::execute(
        world,
        entry,
        payload,
        config,
        config.domain().unwrap(),
        budget,
        StorageReadPolicy::Abstract,
    )
}

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: config.domain().unwrap(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

fn assert_boundary(result: &Execution, reason: FrontierReason) {
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, reason);
    assert!(
        result.outcomes.is_empty(),
        "resource failure is not a terminal"
    );
    assert!(result.successors.is_empty());
    assert!(result.completed_calls.is_empty());
}

fn assert_contains(data: &ByteArray, concrete: &[u8], domain: Domain) {
    assert!(data.len().contains(U256::from(concrete.len())));
    for (index, byte) in concrete.iter().enumerate() {
        assert!(
            data.read_byte_checked(&value(index), domain)
                .unwrap()
                .contains(U256::from(*byte)),
            "byte {index} excludes {byte}"
        );
    }
    assert!(data.byte_at(concrete.len(), domain).contains(U256::ZERO));
}

fn modexp_input() -> Vec<u8> {
    let mut input = vec![0; 96];
    input[31] = 1;
    input[63] = 1;
    input[95] = 1;
    input.extend([2, 5, 13]); // 2^5 mod 13 = 6, one output byte.
    input
}

fn sample(id: &PrecompileId) -> Vec<u8> {
    match id {
        PrecompileId::Sha256 | PrecompileId::Ripemd160 | PrecompileId::Identity => {
            b"hello".to_vec()
        }
        PrecompileId::ModExp => modexp_input(),
        PrecompileId::Blake2F => vec![0; 213], // zero rounds, valid final flag.
        PrecompileId::Bls12G1Add => vec![0; 256],
        PrecompileId::Bls12G1Msm => vec![0; 160],
        PrecompileId::Bls12G2Add => vec![0; 512],
        PrecompileId::Bls12G2Msm => vec![0; 288],
        PrecompileId::Bls12Pairing => vec![0; 384],
        PrecompileId::Bls12MapFpToGp1 => vec![0; 64],
        PrecompileId::Bls12MapFp2ToGp2 => vec![0; 128],
        _ => Vec::new(),
    }
}

#[test]
fn concrete_fast_path_matches_every_pinned_fork_table_entry() {
    let config = config();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        assert!(!contains(fork, address(0x200)));
        assert_eq!(contains(fork, address(0x100)), fork == Fork::Osaka);
        assert_eq!(contains(fork, address(0x0b)), fork != Fork::Cancun);
        for native in table(fork).inner().values() {
            for input in [Vec::new(), sample(native.id())] {
                let expected = native.execute(&input, u64::MAX, 0).unwrap();
                let (world, entry, payload) =
                    fixture(*native.address(), ByteArray::exact(&input), fork, &config);
                let result = run(
                    &world,
                    &entry,
                    payload,
                    &config,
                    &mut WorkBudget::new(config.max_work),
                );
                assert!(result.frontiers.is_empty(), "{fork} {:?}", native.id());
                assert_eq!(result.outcomes.len(), 1);
                let expected_kind = match expected.status {
                    PrecompileStatus::Success => OutcomeKind::Return,
                    PrecompileStatus::Revert => OutcomeKind::Revert,
                    PrecompileStatus::Halt(_) => OutcomeKind::Failure,
                };
                assert_eq!(result.outcomes[0].kind, expected_kind);
                let expected_bytes: &[u8] = if expected_kind == OutcomeKind::Failure {
                    &[]
                } else {
                    &expected.bytes
                };
                assert_eq!(
                    result.outcomes[0].data.exact_bytes().as_deref(),
                    Some(expected_bytes),
                    "{fork} {:?}",
                    native.id()
                );
            }
        }
    }
}

#[test]
fn symbolic_outputs_include_provider_successes_and_invalid_inputs_for_all_forks() {
    let config = config();
    let domain = config.domain().unwrap();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for native in table(fork).inner().values() {
            let (world, entry, payload) =
                fixture(*native.address(), ByteArray::unknown(), fork, &config);
            let result = run(
                &world,
                &entry,
                payload,
                &config,
                &mut WorkBudget::new(config.max_work),
            );
            assert!(
                result.frontiers.is_empty(),
                "{fork} {:?}: {:?}",
                native.id(),
                result.frontiers
            );
            assert_eq!(result.outcomes.len(), 3);
            let returned = result
                .outcomes
                .iter()
                .find(|o| o.kind == OutcomeKind::Return)
                .unwrap();
            let reverted = result
                .outcomes
                .iter()
                .find(|o| o.kind == OutcomeKind::Revert)
                .unwrap();
            let failed = result
                .outcomes
                .iter()
                .find(|o| o.kind == OutcomeKind::Failure)
                .unwrap();
            assert_eq!(failed.data.exact_bytes(), Some(Vec::new()));
            assert!(reverted.data.len().contains(U256::MAX));
            for input in [Vec::new(), sample(native.id()), vec![255; 96]] {
                let concrete = native.execute(&input, u64::MAX, 0).unwrap();
                match concrete.status {
                    PrecompileStatus::Success => {
                        assert_contains(&returned.data, &concrete.bytes, domain)
                    }
                    PrecompileStatus::Revert => {
                        assert_contains(&reverted.data, &concrete.bytes, domain)
                    }
                    PrecompileStatus::Halt(_) => {
                        assert_eq!(failed.data.len().singleton(), Some(U256::ZERO))
                    }
                }
            }
            // Available CALL gas is abstract. Native empty-input special cases
            // need not all halt at zero gas; cover the actual pinned status.
            let no_gas = native.execute(&sample(native.id()), 0, 0).unwrap();
            let (included, data) = match no_gas.status {
                PrecompileStatus::Success => (OutcomeKind::Return, &returned.data),
                PrecompileStatus::Revert => (OutcomeKind::Revert, &reverted.data),
                PrecompileStatus::Halt(_) => (OutcomeKind::Failure, &failed.data),
            };
            assert!(
                result
                    .outcomes
                    .iter()
                    .any(|outcome| outcome.kind == included),
                "missing {fork} {:?} zero-gas status {included:?}",
                native.id()
            );
            if included != OutcomeKind::Failure {
                assert_contains(data, &no_gas.bytes, domain);
            } else {
                assert_eq!(data.len().singleton(), Some(U256::ZERO));
            }
            // A spurious empty success alone cannot cover nonempty outputs.
            if !matches!(native.id(), PrecompileId::EcRec | PrecompileId::P256Verify) {
                assert_ne!(returned.data.len().singleton(), Some(U256::ZERO));
            }
        }
    }
}

#[test]
fn finite_abstract_inputs_cover_both_valid_and_invalid_native_cases() {
    let config = config();
    let domain = config.domain().unwrap();
    for (target, valid, other, other_success) in [
        (6, vec![0; 128], vec![255; 128], false),
        (5, modexp_input(), vec![255; 96], false),
        (4, b"hello".to_vec(), b"different".to_vec(), true),
    ] {
        let input = ByteArray::exact(&valid).join(&ByteArray::exact(&other), domain);
        let (world, entry, payload) = fixture(address(target), input, Fork::Osaka, &config);
        let result = run(
            &world,
            &entry,
            payload,
            &config,
            &mut WorkBudget::new(config.max_work),
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(result.outcomes.len(), 3);
        let native = table(Fork::Osaka).get(&address(target)).unwrap();
        for (input, success) in [(valid, true), (other, other_success)] {
            let concrete = native.execute(&input, u64::MAX, 0).unwrap();
            assert_eq!(
                matches!(concrete.status, PrecompileStatus::Success),
                success
            );
            let kind = if success {
                OutcomeKind::Return
            } else {
                OutcomeKind::Failure
            };
            let outcome = result.outcomes.iter().find(|o| o.kind == kind).unwrap();
            assert_contains(&outcome.data, &concrete.bytes, domain);
        }
    }
}

#[test]
fn signature_success_buffers_cover_empty_and_word_and_report_typed_limits() {
    let config = config();
    let input = ByteArray::unknown();
    for id in [PrecompileId::EcRec, PrecompileId::P256Verify] {
        let data = success_data(&input, &id, config.domain().unwrap()).unwrap();
        assert!(data.len().contains(U256::ZERO));
        assert!(data.len().contains(U256::from(32)));
        assert_contains(&data, &[], config.domain().unwrap());
        assert_contains(&data, &[255; 32], config.domain().unwrap());
        let mut limited = config.clone();
        limited.analysis.relations.max_nodes = 1;
        let (_, _, payload) = fixture(address(1), input.clone(), Fork::Cancun, &config);
        let mut result = empty_execution(payload);
        summarize(
            &mut result,
            &id,
            &mut context(&limited, &mut WorkBudget::new(limited.max_work)),
        );
        assert_boundary(
            &result,
            FrontierReason::Relations(QueryReason::ExpressionLimit),
        );
    }
}

#[test]
fn identity_keeps_full_symbolic_content_and_huge_length_without_materializing() {
    let config = ExecutionConfig {
        max_memory_bytes: 8,
        max_work: 50_000,
        ..config()
    };
    let domain = config.domain().unwrap();
    let unknown = ByteArray::unknown();
    let huge = unknown
        .slice(&value(0), &value(usize::MAX), usize::MAX, domain)
        .unwrap();
    let mut observed = unknown.slice(&value(0), &value(3), 3, domain).unwrap();
    observed
        .write_byte(&value(1), &value(42), 3, domain)
        .unwrap();
    for input in [
        unknown,
        huge,
        observed,
        ByteArray::exact(b"known but above extraction cap"),
    ] {
        let (world, entry, payload) = fixture(address(4), input, Fork::Osaka, &config);
        let expected = payload.active().calldata.clone();
        let mut budget = WorkBudget::new(config.max_work);
        let result = run(&world, &entry, payload, &config, &mut budget);
        assert!(result.frontiers.is_empty());
        let returned = result
            .outcomes
            .iter()
            .find(|o| o.kind == OutcomeKind::Return)
            .unwrap();
        assert_eq!(
            returned.data, expected,
            "preserve source, length and all facts"
        );
        assert!(budget.used() < 50_000);
    }
}

#[test]
fn a_large_known_length_with_unknown_content_needs_no_full_materialization_fee() {
    let config = ExecutionConfig {
        max_memory_bytes: 1_000_000,
        max_work: 1_000_000,
        ..config()
    };
    let input = ByteArray::unknown()
        .slice(
            &value(0),
            &value(1_000_000),
            1_000_000,
            config.domain().unwrap(),
        )
        .unwrap();
    let (world, entry, payload) = fixture(address(2), input, Fork::Osaka, &config);
    let mut budget = WorkBudget::new(config.max_work);
    let result = run(&world, &entry, payload, &config, &mut budget);
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 3);
    assert!(!budget.exhausted());
    assert!(budget.used() < 1_000_000);
    assert!(result.outcomes.iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome.data.len().singleton() == Some(U256::from(32))
    }));
}

#[test]
fn modexp_success_length_is_not_capped_by_the_analysis_memory_policy() {
    let config = ExecutionConfig {
        max_memory_bytes: 8,
        ..config()
    };
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let (world, entry, payload) = fixture(address(5), ByteArray::unknown(), fork, &config);
        let result = run(
            &world,
            &entry,
            payload,
            &config,
            &mut WorkBudget::new(config.max_work),
        );
        assert!(result.frontiers.is_empty());
        let returned = result
            .outcomes
            .iter()
            .find(|o| o.kind == OutcomeKind::Return)
            .unwrap();
        assert!(returned.data.len().contains(U256::from(1_000_000)));
        assert!(returned.data.len().contains(U256::MAX));
        assert!(
            returned
                .data
                .byte_at(100, config.domain().unwrap())
                .contains(U256::from(255))
        );
    }
}

fn entered_child(target: u64, config: &ExecutionConfig) -> (World, Entry, MachinePayload) {
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "symbolic native continuation");
    // Write a caller slot, capture a symbolic input word, initialize the output
    // word to 42, then CALL the native target with value 3 and a 32-byte output.
    let code = format!("60075f555f355f52602a6020526020602060205f600360{target:02x}5af100");
    let mut caller = Account::from_hex(&code, Fork::Osaka).unwrap();
    caller.balance = value(100);
    world.insert(owner, caller).unwrap();
    let mut callee = Account::from_hex("00", Fork::Osaka).unwrap();
    callee.balance = value(50);
    world.insert(address(target), callee).unwrap();
    let entry = Entry::concrete(owner, address(0x900), value(0), ByteArray::unknown());
    let payload = initial(&world, &entry, config.domain().unwrap()).unwrap();
    let result = run(
        &world,
        &entry,
        payload,
        config,
        &mut WorkBudget::new(config.max_work),
    );
    assert!(result.frontiers.is_empty());
    let child = result
        .successors
        .into_iter()
        .find(|s| s.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    assert_eq!(child.active().code, FrameCode::Precompile(address(target)));
    assert_eq!(child.call_stack.depth(), 2);
    (world, entry, child)
}

#[test]
fn symbolic_child_continues_with_full_returndata_and_checkpoint_rollback() {
    let config = config();
    let domain = config.domain().unwrap();
    for target in [4, 6, 5] {
        let (world, entry, child) = entered_child(target, &config);
        let checkpoint = child.active().saved_store.state().clone();
        assert_eq!(
            checkpoint
                .read(address(0x101), &value(0), domain)
                .singleton(),
            Some(U256::from(7))
        );
        assert_eq!(
            child.store.read_balance(address(0x101)).singleton(),
            Some(U256::from(97))
        );
        let entered_store = child.store.clone();
        let input = child.active().calldata.clone();
        let result = run(
            &world,
            &entry,
            child,
            &config,
            &mut WorkBudget::new(config.max_work),
        );
        assert!(result.frontiers.is_empty());
        assert!(
            result.outcomes.is_empty(),
            "child completion must resume the caller"
        );
        assert_eq!(result.successors.len(), 3);
        assert_eq!(result.completed_calls.len(), 3);
        for successor in result.successors {
            let parent = successor.payload.active();
            assert_eq!(successor.payload.call_stack.depth(), 1);
            assert_eq!(parent.key.basic_block_index, 1);
            if successor.kind == MachineEdgeKind::Return {
                assert_eq!(
                    successor.payload.store, entered_store,
                    "native summary must be pure"
                );
                assert_eq!(
                    parent.stack.last().unwrap().singleton(),
                    Some(U256::from(1))
                );
                match target {
                    4 => assert_eq!(parent.returndata, input),
                    6 => assert_eq!(parent.returndata.len().singleton(), Some(U256::from(64))),
                    5 => assert!(parent.returndata.len().contains(U256::MAX)),
                    _ => unreachable!(),
                }
            } else {
                assert_eq!(successor.payload.store, checkpoint);
                assert_eq!(parent.stack.last().unwrap().singleton(), Some(U256::ZERO));
                if successor.kind == MachineEdgeKind::Failure {
                    assert_eq!(parent.returndata.exact_bytes(), Some(Vec::new()));
                    assert_eq!(
                        parent.memory.read_word(&value(32), domain).singleton(),
                        Some(U256::from(42))
                    );
                } else {
                    assert_eq!(successor.kind, MachineEdgeKind::Revert);
                    assert!(parent.returndata.len().contains(U256::MAX));
                }
            }
        }
    }
}

#[test]
fn child_finish_work_refusal_keeps_partial_continuations_open() {
    let config = config();
    let (world, entry, child) = entered_child(6, &config);
    let mut budget = WorkBudget::new(config.max_work);
    let complete = run(&world, &entry, child.clone(), &config, &mut budget);
    assert!(complete.frontiers.is_empty());
    assert_eq!(complete.successors.len(), 3);
    let partial = run(
        &world,
        &entry,
        child.clone(),
        &config,
        &mut WorkBudget::new(budget.used() - 1),
    );
    assert_eq!(partial.frontiers.len(), 1);
    assert_eq!(partial.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(partial.successors.len(), 2);
    assert_eq!(partial.completed_calls.len(), 2);
    assert!(partial.outcomes.is_empty());
    assert!(
        partial
            .successors
            .iter()
            .all(|s| s.kind != MachineEdgeKind::Failure)
    );
    assert_eq!(partial.payload, child);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        let result = run(
            &world,
            &entry,
            child,
            &config,
            &mut WorkBudget::new(config.max_work),
        );
        assert_boundary(&result, FrontierReason::Relations(QueryReason::Cancelled));
    });
}

#[test]
fn whole_engine_closes_symbolic_native_roots_and_executes_the_parent_continuation() {
    let config = config();
    for target in [2, 4, 5, 6, 9] {
        let (world, entry, _) =
            fixture(address(target), ByteArray::unknown(), Fork::Osaka, &config);
        let graph = analysis::analyze_world(world, entry, config.clone()).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        for kind in [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ] {
            assert!(graph.outcomes().iter().any(|outcome| outcome.kind == kind));
        }
    }
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "native parent execution");
    // Store the resumed CALL flag and RETURNDATASIZE after the copied output.
    world
        .insert(
            owner,
            Account::from_hex(
                "5f355f526020602060205f5f60045af16040523d60605260606020f3",
                Fork::Osaka,
            )
            .unwrap(),
        )
        .unwrap();
    let entry = Entry::concrete(owner, address(0x900), value(0), ByteArray::unknown());
    let graph = analysis::analyze_world(world, entry, config.clone()).unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(
        graph
            .states()
            .iter()
            .any(|state| state.entry.active().code == FrameCode::Precompile(address(4)))
    );
    let domain = config.domain().unwrap();
    assert!(graph.outcomes().iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome
                .data
                .read_word(&value(0), domain)
                .contains(U256::from(7))
            && outcome
                .data
                .read_word(&value(32), domain)
                .contains(U256::from(1))
            && outcome
                .data
                .read_word(&value(64), domain)
                .contains(U256::from(32))
    }));
    assert!(graph.outcomes().iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome
                .data
                .read_word(&value(32), domain)
                .contains(U256::ZERO)
    }));
}

#[test]
fn summary_admission_and_input_retention_refusals_have_no_fake_terminals() {
    let config = config();
    let (world, entry, payload) = fixture(address(6), ByteArray::unknown(), Fork::Osaka, &config);
    let mut full_budget = WorkBudget::new(config.max_work);
    let full = run(&world, &entry, payload.clone(), &config, &mut full_budget);
    assert_eq!(full.outcomes.len(), 3);
    let scan = payload
        .active()
        .calldata
        .metadata_len()
        .saturating_add(payload.active().calldata.state_root().work_size());
    for limit in [0, scan, full_budget.used() - 1] {
        let mut budget = WorkBudget::new(limit);
        let result = run(&world, &entry, payload.clone(), &config, &mut budget);
        assert_boundary(&result, FrontierReason::Work);
        assert_eq!(result.payload, payload);
        assert!(budget.exhausted());
        assert!(budget.used() <= limit);
    }
    let mut limited = config.clone();
    limited.analysis.relations.max_nodes = 1;
    let result = run(
        &world,
        &entry,
        payload.clone(),
        &limited,
        &mut WorkBudget::new(limited.max_work),
    );
    assert_boundary(
        &result,
        FrontierReason::Relations(QueryReason::ExpressionLimit),
    );
    let mut result = empty_execution(payload);
    summarize(
        &mut result,
        &PrecompileId::Bn254Add,
        &mut context(&limited, &mut WorkBudget::new(limited.max_work)),
    );
    assert_boundary(
        &result,
        FrontierReason::Relations(QueryReason::ExpressionLimit),
    );
}

#[test]
fn huge_native_and_materialization_work_use_summaries_even_with_a_large_ledger() {
    let mut config = config();
    config.max_work = 1_000_000_000_000;
    config.max_memory_bytes = 1_000_000;
    config.analysis.max_constants = 8;
    let mut blake = vec![0; 213];
    blake[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(cost(address(9), &blake, config.max_memory_bytes).unwrap() > MAX_EXACT_PRECOMPILE_WORK);
    let concrete = ByteArray::exact(&blake);
    // Establish that the native-quantum case gets past exact materialization,
    // rather than accidentally testing only the other admission boundary.
    assert_eq!(
        materialize(
            &concrete,
            &mut context(&config, &mut WorkBudget::new(config.max_work)),
        )
        .unwrap(),
        Some(blake)
    );
    for (target, input) in [(9, concrete), (2, ByteArray::exact(&vec![0; 262_144]))] {
        let (_, _, payload) = fixture(address(target), input, Fork::Osaka, &config);
        let mut result = empty_execution(payload);
        let mut budget = WorkBudget::new(config.max_work);
        execute_with(
            &mut result,
            Fork::Osaka,
            address(target),
            &mut context(&config, &mut budget),
            |_, _| panic!("an over-quantum native must not execute"),
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(result.outcomes.len(), 3);
        assert!(!budget.exhausted());
        let expected_length = if target == 9 { 64 } else { 32 };
        assert!(result.outcomes.iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome.data.len().singleton() == Some(U256::from(expected_length))
        }));
    }
}

#[test]
fn exact_native_admission_keeps_cheap_computation_and_sticky_resource_refusal() {
    let config = config();
    let (_, _, payload) = fixture(address(2), ByteArray::exact(b"hello"), Fork::Osaka, &config);
    let mut result = empty_execution(payload.clone());
    let mut called = false;
    execute_with(
        &mut result,
        Fork::Osaka,
        address(2),
        &mut context(&config, &mut WorkBudget::new(config.max_work)),
        |native, input| {
            called = true;
            native.execute(input, u64::MAX, 0)
        },
    );
    assert!(called);
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    let mut budget = WorkBudget::new(config.max_work);
    assert!(!budget.charge(config.max_work + 1));
    let mut result = empty_execution(payload);
    execute_with(
        &mut result,
        Fork::Osaka,
        address(2),
        &mut context(&config, &mut budget),
        |_, _| panic!("a refused ledger must not execute native work"),
    );
    assert_boundary(&result, FrontierReason::Work);
    assert_eq!(budget.used(), 0);
}

#[test]
fn expensive_concrete_crypto_is_optional_and_cancellation_stays_a_frontier() {
    let config = config();
    let mut blake = vec![0; 213];
    blake[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    let (world, entry, payload) =
        fixture(address(9), ByteArray::exact(&blake), Fork::Osaka, &config);
    let result = run(
        &world,
        &entry,
        payload,
        &config,
        &mut WorkBudget::new(config.max_work),
    );
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 3);
    assert!(result.outcomes.iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome.data.len().singleton() == Some(U256::from(64))
    }));
    let mut oversized = modexp_input();
    oversized[64..96].copy_from_slice(&U256::MAX.to_be_bytes::<32>());
    let (world, entry, payload) = fixture(
        address(5),
        ByteArray::exact(&oversized),
        Fork::Osaka,
        &config,
    );
    let result = run(
        &world,
        &entry,
        payload,
        &config,
        &mut WorkBudget::new(config.max_work),
    );
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 3);
    assert!(result.outcomes.iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return && outcome.data.len().contains(U256::MAX)
    }));
    for target in [4, 6] {
        let (world, entry, payload) =
            fixture(address(target), ByteArray::unknown(), Fork::Osaka, &config);
        let control = Control::new(Default::default());
        control.cancellation().cancel();
        control.scope(|| {
            let result = run(
                &world,
                &entry,
                payload,
                &config,
                &mut WorkBudget::new(config.max_work),
            );
            assert_boundary(&result, FrontierReason::Relations(QueryReason::Cancelled));
        });
        assert!(matches!(
            analysis::analyze_world_with_control(world, entry, config.clone(), &control),
            Err(analysis::ControlledAnalysisError::Cancelled)
        ));
    }
}

#[test]
fn fatal_backend_and_output_retention_errors_never_turn_into_native_failure() {
    let config = config();
    let (_, _, payload) = fixture(address(6), ByteArray::exact(&[]), Fork::Osaka, &config);
    let mut result = empty_execution(payload.clone());
    finish_native(
        &mut result,
        address(6),
        Err(PrecompileError::Fatal("backend refused".into())),
        &mut context(&config, &mut WorkBudget::new(config.max_work)),
    );
    assert_boundary(&result, FrontierReason::Precompile(address(6)));
    let mut result = empty_execution(payload.clone());
    finish_native(
        &mut result,
        address(6),
        Ok(PrecompileOutput::new(0, vec![7; 64].into(), 0)),
        &mut context(&config, &mut WorkBudget::new(0)),
    );
    assert_boundary(&result, FrontierReason::Work);
    let limited = ExecutionConfig {
        max_memory_bytes: 1,
        ..config.clone()
    };
    let mut result = empty_execution(payload.clone());
    finish_native(
        &mut result,
        address(6),
        Ok(PrecompileOutput::new(0, vec![7; 64].into(), 0)),
        &mut context(&limited, &mut WorkBudget::new(limited.max_work)),
    );
    assert_boundary(&result, FrontierReason::Memory);
    let mut result = empty_execution(payload.clone());
    finish_native(
        &mut result,
        address(6),
        Ok(PrecompileOutput::halt(PrecompileHalt::OutOfGas, 0)),
        &mut context(&config, &mut WorkBudget::new(config.max_work)),
    );
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
    assert_eq!(result.outcomes[0].data.exact_bytes(), Some(Vec::new()));
    let mut result = empty_execution(payload);
    finish_native(
        &mut result,
        address(6),
        Ok(PrecompileOutput::revert(0, vec![7; 64].into(), 0)),
        &mut context(&config, &mut WorkBudget::new(config.max_work)),
    );
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].kind, OutcomeKind::Revert);
    assert_eq!(result.outcomes[0].data.exact_bytes(), Some(vec![7; 64]));
}

#[test]
fn engine_work_refusal_cannot_report_a_closed_symbolic_call() {
    let config = ExecutionConfig {
        max_work: 1,
        ..config()
    };
    let (world, entry, _) = fixture(address(4), ByteArray::unknown(), Fork::Osaka, &config);
    let graph = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(
        graph
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(graph.outcomes().is_empty());
}
