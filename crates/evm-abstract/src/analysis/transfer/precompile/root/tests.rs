//! Compare the Store-only path with ordinary completion, preserving captured symbols.

use super::*;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, MachineEdgeKind, MachineKey,
        control::Control,
        machine::fixtures,
        transfer::{StorageReadPolicy, empty_execution, initial},
    },
    domain::{Domain, relational::QueryReason, symbolic::ExprId},
    world::{AbstractLog, Account, Code, Entry, Existence, LogKey, LogSourceKind, Symbol, World},
};
use alloy_primitives::{Address, U256, keccak256};
use std::collections::BTreeMap;

const KINDS: [OutcomeKind; 3] = [
    OutcomeKind::Return,
    OutcomeKind::Revert,
    OutcomeKind::Failure,
];

fn config() -> ExecutionConfig {
    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    config
}

fn address(number: u64) -> Address {
    fixtures::address(number)
}

fn word(number: u64) -> AbstractValue {
    fixtures::word(number)
}

fn context<'a>(
    config: &'a ExecutionConfig,
    budget: &'a mut WorkBudget,
    procedure: bool,
) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: config.domain().unwrap(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure,
        discover_code: false,
    }
}

fn native_payload(target: u64, config: &ExecutionConfig) -> MachinePayload {
    native_input_payload(target, ByteArray::unknown(), config)
}

fn native_input_payload(target: u64, input: ByteArray, config: &ExecutionConfig) -> MachinePayload {
    let mut world = World::new(Fork::Osaka, "native root completion");
    world.insert(address(target), Account::empty()).unwrap();
    let entry = Entry::concrete(address(target), address(0x900), word(0), input);
    initial(&world, &entry, config.domain().unwrap()).unwrap()
}

#[derive(Clone, Copy, Debug)]
pub(in crate::analysis::transfer::precompile) struct AdmissionReceipts {
    pub(in crate::analysis::transfer::precompile) key_census: usize,
    pub(in crate::analysis::transfer::precompile) key_admitted: usize,
    pub(in crate::analysis::transfer::precompile) completion_census: usize,
    pub(in crate::analysis::transfer::precompile) construction_ready: usize,
    pub(in crate::analysis::transfer::precompile) buffer_discovery: [usize; 3],
    pub(in crate::analysis::transfer::precompile) validation_ready: usize,
}

/// Cumulative receipts from the production quotes, excluding root finalization.
/// Output Fresh identities are irrelevant to their complete representation fees.
pub(in crate::analysis::transfer::precompile) fn admission_receipts(
    payload: &MachinePayload,
    id: &PrecompileId,
    config: &ExecutionConfig,
) -> AdmissionReceipts {
    let mut budget = WorkBudget::new(config.max_work);
    let key = KeyWork::measure(payload, &mut budget).unwrap();
    let key_census = budget.used();
    assert!(reserve(&mut budget, key.total()).is_some());
    let key_admitted = budget.used();
    let completion =
        CompletionWork::measure(payload, id, &mut context(config, &mut budget, false)).unwrap();
    let completion_census = budget.used();
    assert!(reserve(&mut budget, completion.total()).is_some());
    let construction_ready = budget.used();
    let data = [
        success_data(&payload.active().calldata, id, config.domain().unwrap()).unwrap(),
        ByteArray::unknown_checked().unwrap(),
        ByteArray::empty(),
    ];
    let mut buffer_discovery = [0; 3];
    let mut validation = 0usize;
    for (receipt, data) in buffer_discovery.iter_mut().zip(&data) {
        let work = BufferWork::measure(data, &mut budget).unwrap();
        *receipt = budget.used();
        validation = validation.saturating_add(work.total());
    }
    assert!(reserve(&mut budget, validation).is_some());
    AdmissionReceipts {
        key_census,
        key_admitted,
        completion_census,
        construction_ready,
        buffer_discovery,
        validation_ready: budget.used(),
    }
}

fn joined_payload(target: u64, config: &ExecutionConfig) -> MachinePayload {
    let domain = config.domain().unwrap();
    let owner = address(0x201);
    let maybe_empty = address(0x202);
    let input = AbstractValue::top().with_expression(ExprId::input(0x513, Symbol::CallValue));
    let fresh = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let mut world = World::new(Fork::Osaka, "joint native root Store");
    let mut account = Account::from_hex("00", world.fork()).unwrap();
    account.balance = word(1);
    account.storage.insert(U256::ZERO, input.clone());
    world.insert(owner, account).unwrap();
    let mut maybe = Account::empty();
    maybe.balance = domain
        .join(&word(0), &word(1))
        .with_expression(ExprId::fresh().unwrap());
    world.insert(maybe_empty, maybe).unwrap();
    world.insert(address(3), Account::empty()).unwrap();
    world.insert(address(0x203), Account::absent()).unwrap();
    world.insert(address(target), Account::empty()).unwrap();
    let entry = Entry::concrete(
        address(target),
        address(0x900),
        word(0),
        ByteArray::unknown(),
    );
    let mut payload = initial(&world, &entry, domain).unwrap();
    let log_key = LogKey {
        address: owner,
        code_address: owner,
        code_hash: keccak256([0]),
        source_kind: LogSourceKind::Runtime,
        pc: 0,
    };
    let mut left = payload.store.clone();
    let mut right = payload.store.clone();
    for (store, value) in [(&mut left, &input), (&mut right, &fresh)] {
        store.write_checked(owner, &word(1), value, domain).unwrap();
        store
            .write_transient_checked(owner, &word(2), value, domain)
            .unwrap();
        store.write_balance(owner, value.clone());
        store
            .emit_log(
                log_key.clone(),
                AbstractLog {
                    topics: vec![value.clone()],
                    data: payload.active().calldata.clone(),
                },
                domain,
            )
            .unwrap();
    }
    payload.store = left
        .join_guarded(
            &right,
            domain,
            ExprId::fresh().unwrap(),
            domain.spec().relations().expressions(),
        )
        .unwrap();
    payload.store.touch_account(maybe_empty);
    payload.store.touch_account(address(3));
    payload.store.begin_creation(address(0x203));
    payload
}

fn assert_refusal(
    result: &Execution,
    original: &MachinePayload,
    reason: FrontierReason,
    target: Option<MachineKey>,
) {
    assert_eq!(result.payload, *original);
    assert!(result.outcomes.is_empty());
    assert!(result.successors.is_empty());
    assert!(result.completed_calls.is_empty());
    assert!(result.executed_pcs.is_empty());
    assert!(result.instruction_progress.is_empty());
    assert!(result.instruction_values.is_empty());
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].pc, 0);
    assert_eq!(result.frontiers[0].reason, reason);
    assert_eq!(result.frontiers[0].target, target);
    assert_eq!(result.frontiers[0].range, None);
    assert_eq!(result.frontiers[0].payload, None);
}

fn assert_shapes(result: &Execution, original: &MachinePayload, target: u64, domain: Domain) {
    assert_eq!(
        result.outcomes.iter().map(|o| o.kind).collect::<Vec<_>>(),
        KINDS
    );
    for outcome in &result.outcomes {
        outcome
            .data
            .check_existing(domain.spec().relations().expressions())
            .unwrap();
    }
    let returned = &result.outcomes[0].data;
    match target {
        4 => assert_eq!(*returned, original.active().calldata),
        1 => {
            assert!(returned.len().contains(U256::ZERO));
            assert!(returned.len().contains(U256::from(32)));
        }
        6 => assert_eq!(returned.len().singleton(), Some(U256::from(64))),
        5 => assert!(returned.len().contains(U256::MAX)),
        _ => unreachable!(),
    }
    assert!(result.outcomes[1].data.len().contains(U256::MAX));
    assert_eq!(result.outcomes[2].data, ByteArray::empty());
}

fn assert_ordinary_root_reference(
    observed: &Execution,
    original: &MachinePayload,
    config: &ExecutionConfig,
    procedure: bool,
) {
    assert_eq!(observed.payload, *original);
    assert!(observed.frontiers.is_empty());
    assert!(observed.successors.is_empty());
    assert!(observed.completed_calls.is_empty());
    assert!(observed.executed_pcs.is_empty());
    assert!(observed.instruction_progress.is_empty());
    assert!(observed.instruction_values.is_empty());
    assert!(observed.diagnostics.is_empty());
    let mut expected = empty_execution(original.clone());
    let mut budget = WorkBudget::new(config.max_work);
    // The completion oracle shares complete output buffers. Their semantic
    // shapes are asserted separately, so only cleanup selectors need renaming.
    for outcome in &observed.outcomes {
        calls::finish(
            &mut expected,
            original.clone(),
            outcome.kind,
            outcome.data.clone(),
            &mut context(config, &mut budget, procedure),
            0,
        );
    }
    assert!(expected.frontiers.is_empty());
    assert_eq!(expected.outcomes.len(), observed.outcomes.len());
    let limits = config.domain().unwrap().spec().relations().expressions();
    let mut bound = original.fresh_leaves();
    for outcome in &observed.outcomes {
        bound.extend(outcome.data.fresh_leaves());
    }
    let mut renaming = BTreeMap::new();
    for (expected, observed) in expected.outcomes.iter().zip(&observed.outcomes) {
        let left = expected
            .store
            .fresh_leaves()
            .difference(&bound)
            .cloned()
            .collect::<Vec<_>>();
        let right = observed
            .store
            .fresh_leaves()
            .difference(&bound)
            .cloned()
            .collect::<Vec<_>>();
        // The sole MAY-empty owner is 0x202. Every other touched owner has
        // known empty code/nonce/balance or a nonempty creation nonce.
        assert_eq!(
            left.len(),
            usize::from(!procedure && expected.kind == OutcomeKind::Return),
        );
        assert_eq!(left.len(), right.len());
        renaming.extend(left.into_iter().zip(right));
    }
    // Use one map across all three complete Stores and their data, keeping
    // captured Input/Fresh leaves and cross-field guarded correlations fixed.
    for (mut expected, observed) in expected.outcomes.into_iter().zip(&observed.outcomes) {
        expected.store.rename_fresh(&renaming, limits).unwrap();
        expected.data.rename_fresh(&renaming, limits).unwrap();
        assert_eq!(expected.kind, observed.kind);
        assert_eq!(expected.data, observed.data);
        assert_eq!(expected.store, observed.store);
    }
}

#[test]
fn native_root_shapes_match_ordinary_completion_for_true_and_relative_roots() {
    let config = config();
    for procedure in [false, true] {
        for target in [4, 1, 6, 5] {
            // Both transaction and procedure roots use the same root role;
            // context controls finalization versus relative checkpoint restore.
            let payload = joined_payload(target, &config);
            assert!(applicable(&payload));
            let mut result = empty_execution(payload.clone());
            let mut budget = WorkBudget::new(config.max_work);
            super::super::summarize(
                &mut result,
                super::super::table(Fork::Osaka)
                    .get(&address(target))
                    .unwrap()
                    .id(),
                &mut context(&config, &mut budget, procedure),
            );
            assert_shapes(&result, &payload, target, config.domain().unwrap());
            assert_ordinary_root_reference(&result, &payload, &config, procedure);
            for outcome in &result.outcomes {
                assert_eq!(
                    outcome.store.touched_in_transaction(address(3)),
                    Some(procedure)
                );
                if procedure && outcome.kind == OutcomeKind::Return {
                    assert_eq!(outcome.store, payload.store);
                }
                if !procedure {
                    assert!(outcome.store.transient_slots().is_empty());
                    assert_eq!(
                        outcome.store.created_in_transaction(address(0x203)),
                        Some(false)
                    );
                } else {
                    assert_eq!(
                        outcome.store.existence(address(0x203)),
                        if outcome.kind == OutcomeKind::Return {
                            Existence::Present
                        } else {
                            Existence::Absent
                        },
                    );
                }
            }
            assert!(!budget.exhausted());
        }
    }
}

#[test]
fn root_refusals_distinguish_key_admission_and_keep_exact_phase_receipts() {
    let config = config();
    let payload = native_payload(6, &config);
    let receipts = admission_receipts(&payload, &PrecompileId::Bn254Add, &config);
    for (limit, used, target) in [
        (0, 0, None),
        (receipts.key_census - 1, 0, None),
        (receipts.key_admitted - 1, receipts.key_census, None),
        (
            receipts.key_admitted,
            receipts.key_admitted,
            Some(payload.key()),
        ),
        (
            receipts.construction_ready - 1,
            receipts.completion_census,
            Some(payload.key()),
        ),
        (
            receipts.construction_ready,
            receipts.construction_ready,
            Some(payload.key()),
        ),
        (
            receipts.validation_ready - 1,
            receipts.buffer_discovery[2],
            Some(payload.key()),
        ),
        (
            receipts.validation_ready,
            receipts.validation_ready,
            Some(payload.key()),
        ),
    ] {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(limit);
        summarize(
            &mut result,
            &PrecompileId::Bn254Add,
            &mut context(&config, &mut budget, false),
        );
        assert_refusal(&result, &payload, FrontierReason::Work, target);
        assert!(budget.exhausted());
        assert_eq!(budget.used(), used);
        assert!(budget.used() <= limit);
    }
}

#[test]
fn root_reports_typed_buffer_construction_validation_and_store_refusals() {
    let config = config();
    let payload = native_payload(6, &config);
    let mut limited = config.clone();
    limited.analysis.relations.max_nodes = payload.active().calldata.state_root().nodes();
    let domain = limited.domain().unwrap();
    payload
        .active()
        .calldata
        .check_existing(domain.spec().relations().expressions())
        .unwrap();
    let error =
        success_data(&payload.active().calldata, &PrecompileId::Bn254Add, domain).unwrap_err();
    let reason = super::super::super::errors::bytes(error);
    assert_eq!(
        reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(limited.max_work);
    summarize(
        &mut result,
        &PrecompileId::Bn254Add,
        &mut context(&limited, &mut budget, false),
    );
    assert_refusal(&result, &payload, reason, Some(payload.key()));
    assert_eq!(
        budget.used(),
        admission_receipts(&payload, &PrecompileId::Bn254Add, &config).construction_ready,
    );
    assert!(!budget.exhausted());

    // Return fits the tighter policy. The later Revert buffer must be checked
    // before even this valid first outcome can be published.
    let payload = native_input_payload(4, ByteArray::empty(), &config);
    limited.analysis.relations.max_nodes = 1;
    let domain = limited.domain().unwrap();
    success_data(&payload.active().calldata, &PrecompileId::Identity, domain)
        .unwrap()
        .check_existing(domain.spec().relations().expressions())
        .unwrap();
    assert!(
        ByteArray::unknown()
            .check_existing(domain.spec().relations().expressions())
            .is_err()
    );
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(limited.max_work);
    summarize(
        &mut result,
        &PrecompileId::Identity,
        &mut context(&limited, &mut budget, false),
    );
    assert_refusal(
        &result,
        &payload,
        FrontierReason::Relations(QueryReason::ExpressionLimit),
        Some(payload.key()),
    );
    assert_eq!(
        budget.used(),
        admission_receipts(&payload, &PrecompileId::Identity, &config).validation_ready,
    );
    assert!(!budget.exhausted());

    let mut payload = native_payload(4, &config);
    for index in [0, 1] {
        payload
            .store
            .write_checked(
                address(0x201),
                &word(index),
                &word(7),
                config.domain().unwrap(),
            )
            .unwrap();
    }
    limited.analysis.relations.max_nodes = payload.active().calldata.state_root().nodes();
    let domain = limited.domain().unwrap();
    for data in [
        payload.active().calldata.clone(),
        ByteArray::unknown(),
        ByteArray::empty(),
    ] {
        data.check_existing(domain.spec().relations().expressions())
            .unwrap();
    }
    let error = payload
        .store
        .check_existing(domain.spec().relations().expressions())
        .unwrap_err();
    let expected = super::super::super::errors::store(error);
    let mut reference = empty_execution(payload.clone());
    let mut finish_budget = WorkBudget::new(limited.max_work);
    calls::finish(
        &mut reference,
        payload.clone(),
        OutcomeKind::Return,
        payload.active().calldata.clone(),
        &mut context(&limited, &mut finish_budget, false),
        0,
    );
    assert_refusal(&reference, &payload, expected.clone(), Some(payload.key()));
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(limited.max_work);
    summarize(
        &mut result,
        &PrecompileId::Identity,
        &mut context(&limited, &mut budget, false),
    );
    assert_refusal(&result, &payload, expected, Some(payload.key()));
    assert_eq!(
        budget.used(),
        admission_receipts(&payload, &PrecompileId::Identity, &config).validation_ready
            + finish_budget.used(),
    );
    assert!(!budget.exhausted());
}

#[test]
fn cancelled_root_has_a_typed_no_key_frontier_before_its_first_receipt() {
    let config = config();
    let payload = native_payload(4, &config);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        summarize(
            &mut result,
            &PrecompileId::Identity,
            &mut context(&config, &mut budget, false),
        );
        assert_refusal(
            &result,
            &payload,
            FrontierReason::Relations(QueryReason::Cancelled),
            None,
        );
        assert_eq!(budget.used(), 0);
    });
}

#[test]
fn cancellation_after_key_admission_uses_the_original_key_and_paid_receipt() {
    let config = config();
    let payload = native_payload(4, &config);
    let original_key = payload.key();
    let control = Control::new(Default::default());
    control.scope(|| {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        let key = KeyWork::measure(&payload, &mut budget).unwrap();
        assert!(reserve(&mut budget, key.total()).is_some());
        let paid = budget.used();
        let allowance = ErrorKeyAllowance;
        control.cancellation().cancel();
        assert!(
            CompletionWork::measure(
                &payload,
                &PrecompileId::Identity,
                &mut context(&config, &mut budget, false),
            )
            .is_none()
        );
        assert_eq!(budget.used(), paid);
        allowance.refuse(&mut result, FrontierReason::Work);
        assert_refusal(
            &result,
            &payload,
            FrontierReason::Relations(QueryReason::Cancelled),
            Some(original_key.clone()),
        );
        assert!(budget.exhausted());
        assert_eq!(budget.used(), paid);
    });
}

fn entered_child(code: &str, native: bool, config: &ExecutionConfig) -> MachinePayload {
    let (mut world, entry) = fixtures::fixture(&[(0x101, code)]);
    if native {
        world.insert(address(6), Account::empty()).unwrap();
    } else {
        world
            .insert(address(0x101).create(0), Account::absent())
            .unwrap();
    }
    let payload = initial(&world, &entry, config.domain().unwrap()).unwrap();
    let result = super::super::super::execute(
        &world,
        &entry,
        payload,
        config,
        config.domain().unwrap(),
        &mut WorkBudget::new(config.max_work),
        StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty());
    result
        .successors
        .into_iter()
        .find(|s| s.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload
}

#[test]
fn child_and_runtime_fallback_keep_the_legacy_atomic_summary_price() {
    let config = config();
    let child = entered_child("5f355f526020602060205f5f60065af100", true, &config);
    assert_eq!(child.active().code, FrameCode::Precompile(address(6)));
    let runtime = fixtures::payload(config.domain().unwrap());
    assert_eq!(runtime.active().code, FrameCode::Runtime);
    for payload in [child, runtime] {
        assert!(!applicable(&payload));
        let work = payload
            .work_size()
            .saturating_mul(4)
            .saturating_add(1024)
            .saturating_mul(3);
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(work - 1);
        super::super::summarize(
            &mut result,
            &PrecompileId::Bn254Add,
            &mut context(&config, &mut budget, true),
        );
        assert_refusal(&result, &payload, FrontierReason::Work, Some(payload.key()));
        assert_eq!(budget.used(), 0);

        let mut at_price = empty_execution(payload.clone());
        let mut at_price_budget = WorkBudget::new(work);
        super::super::summarize(
            &mut at_price,
            &PrecompileId::Bn254Add,
            &mut context(&config, &mut at_price_budget, true),
        );
        assert_eq!(at_price_budget.used(), work);
        assert_eq!(at_price.payload, payload);
        if payload.call_stack.active_child().is_none() {
            assert!(at_price.frontiers.is_empty());
            assert_eq!(at_price.outcomes.len(), 3);
        } else {
            assert!(at_price_budget.exhausted());
            assert!(!at_price.frontiers.is_empty());
            assert!(
                at_price
                    .frontiers
                    .iter()
                    .all(|f| f.reason == FrontierReason::Work)
            );
            assert!(at_price.outcomes.is_empty());
        }

        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        super::super::summarize(
            &mut result,
            &PrecompileId::Bn254Add,
            &mut context(&config, &mut budget, true),
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(result.payload, payload);
        let data = if payload.call_stack.active_child().is_some() {
            assert_eq!(result.completed_calls.len(), 3);
            assert_eq!(result.successors.len(), 3);
            assert!(result.outcomes.is_empty());
            result
                .completed_calls
                .iter()
                .map(|c| c.data.clone())
                .collect::<Vec<_>>()
        } else {
            assert_eq!(result.outcomes.len(), 3);
            assert!(result.successors.is_empty());
            result
                .outcomes
                .iter()
                .map(|o| o.data.clone())
                .collect::<Vec<_>>()
        };
        let mut expected = empty_execution(payload.clone());
        let mut finish_budget = WorkBudget::new(config.max_work);
        for (kind, data) in KINDS.into_iter().zip(data) {
            calls::finish(
                &mut expected,
                payload.clone(),
                kind,
                data,
                &mut context(&config, &mut finish_budget, true),
                0,
            );
        }
        assert!(expected.frontiers.is_empty());
        assert_eq!(budget.used(), work + finish_budget.used());
        for (observed, expected) in result.successors.iter().zip(&expected.successors) {
            assert_eq!(observed.kind, expected.kind);
            assert_eq!(observed.payload, expected.payload);
        }
        for (observed, expected) in result.completed_calls.iter().zip(&expected.completed_calls) {
            assert_eq!(observed.payload, expected.payload);
            assert_eq!(observed.kind, expected.kind);
            assert_eq!(observed.data, expected.data);
            assert_eq!(observed.output_store, expected.output_store);
            assert_eq!(observed.output_kind, expected.output_kind);
            assert_eq!(observed.output_data, expected.output_data);
        }
        for (observed, expected) in result.outcomes.iter().zip(&expected.outcomes) {
            assert_eq!(observed.kind, expected.kind);
            assert_eq!(observed.data, expected.data);
            assert_eq!(observed.store, expected.store);
        }
    }
}

#[test]
fn creation_child_still_deploys_code_before_ordinary_completion() {
    let config = config();
    // Copy initcode `PUSH1 1; PUSH0; RETURN`, whose runtime is one STOP byte.
    let child = entered_child("6004600c5f3960045f5ff00060015ff3", false, &config);
    assert_eq!(child.active().code, FrameCode::InitCode);
    assert!(!applicable(&child));
    let created = child
        .call_stack
        .active_child()
        .unwrap()
        .continuation
        .creation
        .unwrap();
    let mut result = empty_execution(child.clone());
    let mut budget = WorkBudget::new(config.max_work);
    calls::finish(
        &mut result,
        child,
        OutcomeKind::Return,
        ByteArray::exact(&[0]),
        &mut context(&config, &mut budget, false),
        0,
    );
    assert!(result.frontiers.is_empty());
    assert!(result.outcomes.is_empty());
    assert_eq!(result.successors.len(), 1);
    let resumed = &result.successors[0].payload;
    assert_eq!(resumed.call_stack.depth(), 1);
    assert_eq!(resumed.active().returndata, ByteArray::empty());
    assert_eq!(
        resumed.active().stack.last().unwrap().singleton(),
        Some(U256::from_be_slice(created.as_slice()))
    );
    assert!(
        matches!(resumed.store.code(created), Some(Code::Runtime(program)) if program.bytes() == [0])
    );
}
