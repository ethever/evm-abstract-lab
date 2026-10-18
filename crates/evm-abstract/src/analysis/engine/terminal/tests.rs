//! Private authority/admission probes; no fallback machine state is fabricated.

use super::*;
use crate::{
    Fork,
    analysis::{ChildFrame, Continuation, transfer},
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprKind, SlotStateId, SlotStateKind, SourceId, StateError},
    },
    world::{Account, Code, Existence},
};
use std::collections::BTreeMap;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn fixture(fork: Fork, is_static: bool) -> (World, Entry, ExecutionConfig, layers::StateFacts) {
    fixture_with_code(fork, is_static, "00")
}

fn fixture_with_code(
    fork: Fork,
    is_static: bool,
    root_code: &str,
) -> (World, Entry, ExecutionConfig, layers::StateFacts) {
    let mut world = World::new(fork, "original root terminal preparation");
    for owner in [address(0x101), address(0x202)] {
        let code = if owner == address(0x101) {
            root_code
        } else {
            "00"
        };
        let mut account = Account::from_hex(code, fork).unwrap();
        account.storage_unknown = true;
        account.balance = word(10);
        account.nonce = word(1);
        world.insert(owner, account).unwrap();
    }
    world.insert(address(3), Account::empty()).unwrap();
    world.insert(address(0x303), Account::empty()).unwrap();
    let mut entry = Entry::new(address(0x101));
    entry.environment.value = word(0);
    entry.environment.is_static = is_static;
    let facts = layers::StateFacts::new(layers::SnapshotId::offline("real session identity"));
    (world, entry, ExecutionConfig::default(), facts)
}

fn ready(
    original: &MachinePayload,
    world: &World,
    entry: &Entry,
    config: &ExecutionConfig,
    facts: &layers::StateFacts,
    budget: &mut WorkBudget,
) -> Result<ReadyRoot, PrepareError> {
    prepare(
        original,
        world,
        entry,
        config,
        config.domain().unwrap(),
        Some((facts.snapshot(), facts.revision())),
        budget,
    )
}

/// One narrow tuple witness assigns a shared source once. Unsupported graph
/// forms decline the witness; numerical marginals cannot establish correlation.
#[derive(Default)]
struct JointInput {
    words: BTreeMap<ExprId, U256>,
    cells: BTreeMap<(SourceId, U256), U256>,
}

impl JointInput {
    fn assign<K: Ord>(map: &mut BTreeMap<K, U256>, key: K, value: U256) -> bool {
        match map.get(&key) {
            Some(previous) => *previous == value,
            None => {
                map.insert(key, value);
                true
            }
        }
    }

    fn expression(&mut self, expression: &ExprId, expected: U256) -> bool {
        match expression.kind() {
            ExprKind::Constant(value) => *value == expected,
            ExprKind::Input(_) | ExprKind::Fresh(_) => {
                Self::assign(&mut self.words, expression.clone(), expected)
            }
            ExprKind::SlotRead { state, key } => key
                .as_constant()
                .is_some_and(|key| self.slot(state, key, expected)),
            _ => false,
        }
    }

    fn slot(&mut self, state: &SlotStateId, key: U256, expected: U256) -> bool {
        match state.kind() {
            SlotStateKind::Zero => expected == U256::ZERO,
            SlotStateKind::Input { source, bindings } => {
                if let Some(binding) = bindings.iter().find(|binding| binding.index == key)
                    && (!binding.numeric.contains(expected)
                        || !self.expression(&binding.value, expected))
                {
                    return false;
                }
                Self::assign(&mut self.cells, (source.clone(), key), expected)
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value,
            } => match written.as_constant() {
                Some(written) if written == key => self.expression(value, expected),
                Some(_) => self.slot(previous, key, expected),
                None => false,
            },
            SlotStateKind::Ite { .. } => false,
        }
    }

    fn storage(&mut self, store: &Store, owner: Address, expected: u64, domain: Domain) -> bool {
        let value = store.read_checked(owner, &word(8), domain).unwrap();
        let expected = U256::from(expected);
        value.contains(expected)
            && value
                .expression()
                .is_some_and(|expr| self.expression(expr, expected))
    }

    fn data(&mut self, data: &ByteArray, expected: &[u8]) -> bool {
        if !data.len().contains(U256::from(expected.len())) {
            return false;
        }
        match data.state_root().kind() {
            ByteStateKind::Exact(bytes) => bytes.as_ref() == expected,
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } if bindings.is_empty() => {
                self.expression(length, U256::from(expected.len()))
                    && expected.iter().enumerate().all(|(index, byte)| {
                        Self::assign(
                            &mut self.cells,
                            (source.clone(), U256::from(index)),
                            U256::from(*byte),
                        )
                    })
            }
            _ => false,
        }
    }
}

#[test]
fn capture_owns_the_actual_lifted_root_and_joint_return_effects() {
    let (world, entry, config, facts) = fixture(Fork::Osaka, false);
    let domain = config.domain().unwrap();
    let original = transfer::initial(&world, &entry, domain).unwrap();
    let unchanged = original.clone();
    let mut budget = WorkBudget::new(usize::MAX);
    let prepared = ready(&original, &world, &entry, &config, &facts, &mut budget).unwrap();
    let used = budget.used();
    let coverage = prepared.publish(RootTerminalTrigger::States).unwrap();
    assert_eq!(budget.used(), used);
    let input = coverage.input();
    assert_eq!(input.snapshot(), facts.snapshot());
    assert_eq!(input.revision(), facts.revision());
    assert_eq!(input.storage_source(), &world.storage_source_token());
    assert_eq!(input.input_scope(), entry.environment.input_scope.id());
    assert_eq!(input.frame(), &original.active().key);
    assert_eq!(input.program(), original.active().program.as_ref().unwrap());
    assert_eq!(input.calldata(), &original.active().calldata);
    assert_eq!(input.call_value(), &original.active().call_value);
    assert_eq!(input.saved_store(), &original.active().saved_store);
    assert_eq!(input.initial_store(), &original.store);
    let returned = &coverage.outcomes()[0];
    assert_eq!(returned.kind(), OutcomeKind::Return);
    let mut tuple = JointInput::default();
    assert!(tuple.storage(input.initial_store(), address(0x101), 7, domain));
    assert!(tuple.storage(input.saved_store().state(), address(0x202), 7, domain));
    assert!(tuple.data(input.calldata(), &[0x11, 0x22]));
    for (owner, value) in [
        (address(0x101), 31),
        (address(0x202), 47),
        (address(0x999), 93),
    ] {
        assert!(tuple.storage(returned.store(), owner, value, domain));
        assert!(returned.store().read_balance(owner).numeric().is_top());
        assert!(returned.store().nonce(owner).numeric().is_top());
        assert!(matches!(
            returned.store().code(owner),
            None | Some(Code::Unknown)
        ));
        assert_eq!(returned.store().existence(owner), Existence::Unknown);
        assert_eq!(
            returned
                .store()
                .read_transient(owner, &word(8), domain)
                .singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(returned.store().created_in_transaction(owner), Some(false));
        assert_eq!(returned.store().pending_destruction(owner), Some(false));
    }
    assert!(tuple.data(returned.data(), &[0xa1, 0xb2, 0xc3]));
    assert!(returned.store().logs_unknown());
    assert!(!tuple.storage(input.initial_store(), address(0x101), 31, domain));
    assert!(!tuple.storage(returned.store(), address(0x101), 47, domain));
    assert!(!tuple.data(input.calldata(), &[0xa1, 0xb2, 0xc3]));
    assert_eq!(original, unchanged);
}

#[test]
fn original_rollback_future_three_touch_and_static_data_share_one_input() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for is_static in [false, true] {
            let (world, entry, config, facts) = fixture(fork, is_static);
            let domain = config.domain().unwrap();
            let original = transfer::initial(&world, &entry, domain).unwrap();
            let coverage = ready(
                &original,
                &world,
                &entry,
                &config,
                &facts,
                &mut WorkBudget::new(usize::MAX),
            )
            .unwrap()
            .publish(RootTerminalTrigger::Transfers)
            .unwrap();
            for (index, kind) in [(1, OutcomeKind::Revert), (2, OutcomeKind::Failure)] {
                let outcome = &coverage.outcomes()[index];
                assert_eq!(outcome.kind(), kind);
                let mut tuple = JointInput::default();
                assert!(tuple.storage(
                    coverage.input().saved_store().state(),
                    entry.address,
                    7,
                    domain
                ));
                assert!(tuple.storage(outcome.store(), entry.address, 7, domain));
                assert!(!tuple.storage(outcome.store(), entry.address, 31, domain));
                assert_eq!(outcome.store().existence(address(3)), Existence::Unknown);
                assert_eq!(
                    outcome.store().existence(address(0x303)),
                    Existence::Present
                );
                assert!(!outcome.store().logs_unknown());
                assert!(outcome.store().possible_logs().is_empty());
                assert_eq!(
                    outcome.store().read_balance(entry.address).singleton(),
                    Some(U256::from(10))
                );
                assert!(tuple.data(
                    outcome.data(),
                    if kind == OutcomeKind::Failure {
                        &[]
                    } else {
                        &[0xaa]
                    }
                ));
                if kind == OutcomeKind::Failure {
                    assert!(!tuple.data(outcome.data(), &[0xaa]));
                }
            }
            if is_static {
                let returned = &coverage.outcomes()[0];
                let mut tuple = JointInput::default();
                assert!(tuple.storage(coverage.input().initial_store(), entry.address, 7, domain));
                assert!(tuple.storage(returned.store(), entry.address, 31, domain));
                assert!(!tuple.storage(
                    coverage.input().initial_store(),
                    entry.address,
                    31,
                    domain
                ));
                assert!(!tuple.storage(returned.store(), entry.address, 7, domain));
                assert!(tuple.data(returned.data(), &[0xbb, 0xcc]));
                assert!(returned.store().logs_unknown());
                assert_eq!(
                    returned.store().existence(address(0x303)),
                    Existence::Unknown
                );
            }
        }
    }
}

#[test]
fn static_return_covers_unlisted_owner_cleanup_under_one_input_valuation() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let (world, entry, config, facts) = fixture(fork, true);
        let domain = config.domain().unwrap();
        let c = address(0x999);
        let d = address(0x998);
        assert!(!world.accounts().contains_key(&c));
        assert!(!world.accounts().contains_key(&d));
        let original = transfer::initial(&world, &entry, domain).unwrap();
        let unchanged = original.clone();
        let coverage = ready(
            &original,
            &world,
            &entry,
            &config,
            &facts,
            &mut WorkBudget::new(usize::MAX),
        )
        .unwrap()
        .publish(RootTerminalTrigger::States)
        .unwrap();
        let input = coverage.input();
        assert_eq!(input.snapshot(), facts.snapshot());
        assert_eq!(input.revision(), facts.revision());
        assert_eq!(input.storage_source(), &world.storage_source_token());
        assert_eq!(input.input_scope(), entry.environment.input_scope.id());
        assert_eq!(input.entry(), &entry);
        assert_eq!(
            serde_json::to_value(input.config()).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
        assert_eq!(input.frame(), &original.active().key);
        assert!(input.frame().is_static);
        assert_eq!(input.program(), original.active().program.as_ref().unwrap());
        assert_eq!(input.calldata(), &original.active().calldata);
        assert_eq!(input.call_value(), &original.active().call_value);
        assert_eq!(input.saved_store(), &original.active().saved_store);
        assert_eq!(input.initial_store(), &original.store);
        let returned = &coverage.outcomes()[0];
        assert_eq!(returned.kind(), OutcomeKind::Return);
        assert!(returned.store().addresses().is_empty());
        let source = |store: &Store, owner| match store.slot_root(owner, false).unwrap().kind() {
            SlotStateKind::Input { source, .. } => source.clone(),
            _ => panic!("expected an immutable input source"),
        };
        assert_eq!(
            source(input.initial_store(), c),
            source(input.saved_store().state(), c)
        );
        assert_ne!(
            source(returned.store(), c),
            source(input.initial_store(), c)
        );
        assert_ne!(
            source(returned.store(), c),
            source(input.saved_store().state(), c)
        );
        assert_ne!(source(returned.store(), c), source(returned.store(), d));

        // Hold the original C[8]=7 and D[8]=11 cells fixed. Separate joint
        // witnesses admit deletion or retention of either omitted owner; they
        // cannot be justified by marginal numeric Top or one global delete flag.
        for (current_c, current_d) in [(0, 0), (0, 11), (7, 0), (7, 11)] {
            let mut tuple = JointInput::default();
            for store in [input.initial_store(), input.saved_store().state()] {
                assert!(tuple.storage(store, c, 7, domain));
                assert!(tuple.storage(store, d, 11, domain));
            }
            assert!(tuple.storage(returned.store(), c, current_c, domain));
            assert!(tuple.storage(returned.store(), d, current_d, domain));
            assert!(!tuple.storage(input.initial_store(), c, 0, domain));
            assert!(!tuple.storage(input.saved_store().state(), c, 0, domain));
            assert!(!tuple.storage(
                returned.store(),
                c,
                if current_c == 0 { 7 } else { 0 },
                domain
            ));
            assert!(!tuple.storage(
                returned.store(),
                d,
                if current_d == 0 { 11 } else { 0 },
                domain
            ));
        }
        assert_eq!(original, unchanged);
        assert_eq!(input.initial_store(), &unchanged.store);
        assert_eq!(input.saved_store(), &unchanged.active().saved_store);
    }
}

#[test]
fn only_genuine_entry_shapes_and_real_identity_can_be_captured() {
    let (world, entry, config, facts) = fixture(Fork::Osaka, false);
    let domain = config.domain().unwrap();
    let original = transfer::initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(matches!(
        prepare(
            &original,
            &world,
            &entry,
            &config,
            domain,
            None,
            &mut budget
        ),
        Err(PrepareError::MissingIdentity)
    ));
    assert_eq!(budget.used(), 0);
    let mut stacked = original.clone();
    stacked.active_mut().stack.push(word(1));
    assert!(matches!(
        ready(&stacked, &world, &entry, &config, &facts, &mut budget),
        Err(PrepareError::Unsupported(UnsupportedRoot::NotEntry))
    ));
    let mut child = original.clone();
    child.call_stack.push_child(ChildFrame {
        state: original.active().clone(),
        continuation: Continuation {
            return_block: Some(0),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
    assert!(matches!(
        ready(&child, &world, &entry, &config, &facts, &mut budget),
        Err(PrepareError::Unsupported(UnsupportedRoot::ActiveChild))
    ));
    for mode in [FrameCode::InitCode, FrameCode::Unknown] {
        let mut unsupported = original.clone();
        unsupported.active_mut().code = mode;
        assert!(matches!(
            ready(&unsupported, &world, &entry, &config, &facts, &mut budget),
            Err(PrepareError::Unsupported(UnsupportedRoot::NotRuntime))
        ));
    }
    let mut view = original.clone();
    view.active_mut().calldata = ByteArray::memory();
    assert!(matches!(
        ready(&view, &world, &entry, &config, &facts, &mut budget),
        Err(PrepareError::Unsupported(UnsupportedRoot::InputByteView))
    ));
    assert!(!budget.exhausted());
}

#[test]
fn exact_total_fee_admits_and_fee_minus_one_declines_without_sticky_work() {
    for is_static in [false, true] {
        let (world, entry, config, facts) = fixture(Fork::Osaka, is_static);
        let original = transfer::initial(&world, &entry, config.domain().unwrap()).unwrap();
        let mut full = WorkBudget::new(usize::MAX);
        let prepared = ready(&original, &world, &entry, &config, &facts, &mut full).unwrap();
        let fee = full.used();
        drop(prepared);
        assert_eq!(full.used(), fee);
        let mut exact = WorkBudget::new(fee + 17);
        assert!(exact.charge(17));
        ready(&original, &world, &entry, &config, &facts, &mut exact).unwrap();
        assert_eq!(exact.used(), fee + 17);
        for maximum in [0, 1, fee / 2, fee - 1] {
            let mut short = WorkBudget::new(maximum);
            assert!(matches!(
                ready(&original, &world, &entry, &config, &facts, &mut short),
                Err(PrepareError::Admission(AdmissionStop::Insufficient))
            ));
            assert!(
                !short.exhausted(),
                "optional limit {maximum} must leave detail runnable"
            );
            if maximum > short.used() {
                assert!(short.charge(1));
            }
        }
        let cancellation = Cancellation::new();
        let mut sticky =
            analysis_control::with_cancellation(&cancellation, || WorkBudget::new(fee));
        assert!(sticky.charge(1));
        assert!(!sticky.charge(fee));
        assert!(sticky.charge(0));
        let used = sticky.used();
        assert!(matches!(
            ready(&original, &world, &entry, &config, &facts, &mut sticky),
            Err(PrepareError::Admission(AdmissionStop::AlreadyExhausted))
        ));
        assert_eq!(sticky.used(), used);
        assert!(matches!(
            available(
                &WorkBudget::new(usize::MAX),
                &Cancellation::new(),
                usize::MAX
            ),
            Err(PrepareError::Admission(AdmissionStop::Overflow))
        ));
    }
}

#[test]
fn prepaid_publish_survives_real_work_exhaustion_but_checks_payer_cancellation() {
    let (world, entry, config, facts) = fixture(Fork::Osaka, false);
    let original = transfer::initial(&world, &entry, config.domain().unwrap()).unwrap();
    let payer = Cancellation::new();
    let unrelated = Cancellation::new();
    unrelated.cancel();
    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
    let prepared = analysis_control::with_cancellation(&unrelated, || {
        ready(&original, &world, &entry, &config, &facts, &mut budget)
    })
    .unwrap();
    let input = std::ptr::from_ref(prepared.anchor.input.as_ref());
    let outcomes = prepared.outcomes.as_ptr();
    assert!(!budget.charge(usize::MAX));
    let used = budget.used();
    assert!(budget.exhausted());
    let coverage = analysis_control::with_cancellation(&unrelated, || {
        prepared.publish(RootTerminalTrigger::Work)
    })
    .unwrap();
    assert_eq!(std::ptr::from_ref(coverage.input()), input);
    assert_eq!(coverage.outcomes().as_ptr(), outcomes);
    assert_eq!(budget.used(), used);
    assert!(budget.exhausted());

    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
    let prepared = ready(&original, &world, &entry, &config, &facts, &mut budget).unwrap();
    let used = budget.used();
    payer.cancel();
    assert!(prepared.publish(RootTerminalTrigger::Work).is_none());
    assert_eq!(budget.used(), used);
    assert!(matches!(
        ready(&original, &world, &entry, &config, &facts, &mut budget),
        Err(PrepareError::Admission(AdmissionStop::Cancelled))
    ));
}

#[test]
fn checked_bytes_can_decline_after_paid_construction_without_mutating_input() {
    let (world, entry, config, facts) = fixture(Fork::Osaka, false);
    let domain = config.domain().unwrap();
    let original = transfer::initial(&world, &entry, domain).unwrap();
    let unchanged = original.clone();
    let mut budget = WorkBudget::new(usize::MAX);
    let cancellation = budget.cancellation();
    let mut calls = 0;
    let result = prepare_with_bytes(
        &original,
        &world,
        &entry,
        &config,
        domain,
        Some((facts.snapshot(), facts.revision())),
        &mut budget,
        &cancellation,
        || {
            calls += 1;
            if calls == 1 {
                ByteArray::unknown_checked()
            } else {
                Err(RangeError::State(StateError::IdentityExhausted))
            }
        },
    );
    assert!(matches!(
        result,
        Err(PrepareError::Bytes(RangeError::State(
            StateError::IdentityExhausted
        )))
    ));
    assert_eq!(calls, 2);
    assert!(budget.used() > 0);
    assert!(!budget.exhausted());
    assert_eq!(original, unchanged);
}

#[test]
fn raw_code_and_offline_label_growth_are_linear_fee_terms() {
    let mut fees = Vec::new();
    for bytes in [1usize, 1025] {
        // World observations are immutable. Construct the intended code once
        // while retaining exactly the same balance, nonce and storage facts.
        let (world, entry, config, _) = fixture_with_code(Fork::Osaka, false, &"00".repeat(bytes));
        let original = transfer::initial(&world, &entry, config.domain().unwrap()).unwrap();
        for label_len in [8usize, 8192] {
            let facts = layers::StateFacts::new(layers::SnapshotId::offline("x".repeat(label_len)));
            let mut budget = WorkBudget::new(usize::MAX);
            ready(&original, &world, &entry, &config, &facts, &mut budget).unwrap();
            fees.push(budget.used());
        }
    }
    let text_delta = 8192 - 8;
    for pair in fees.as_chunks::<2>().0 {
        assert!(pair[1] >= pair[0] + text_delta * 8);
        assert!(pair[1] <= pair[0] + text_delta * 12);
    }
    assert!(fees[2] >= fees[0]);
    assert!(fees[2] <= fees[0] + 1024 * 64);
}

#[test]
fn no_delete_finalizer_guard_funds_actual_owner_local_receipts() {
    for count in [1u64, 8] {
        let (mut world, entry, config, _) = fixture(Fork::Osaka, true);
        let domain = config.domain().unwrap();
        for number in 0..count {
            let mut account = Account::unknown();
            account.balance = domain.join(&word(0), &word(7));
            account.nonce = word(0);
            for slot in 0..4 {
                account
                    .storage
                    .insert(U256::from(slot), domain.join(&word(5), &word(9)));
            }
            world.insert(address(0x400 + number), account).unwrap();
        }
        let original = transfer::initial(&world, &entry, domain).unwrap();
        let saved = &original.active().saved_store;
        let saved_shape = StoreShape::measure(saved.state()).unwrap();
        let owners = world.accounts().len() + 1;
        for (rollback, branch_owners) in [(false, 0), (true, owners)] {
            let mut current = if rollback {
                original.store.clone()
            } else {
                Store::root_terminal_all_effects_checked(domain).unwrap()
            };
            if rollback {
                current.may_touch_account(address(3));
            } else {
                assert!(current.addresses().is_empty());
            }
            let guard = finalization_bound(
                StoreShape::measure(&current).unwrap(),
                rollback.then_some(saved_shape),
                branch_owners,
            );
            let mut budget = WorkBudget::new(guard);
            current
                .finalize_transaction_checked(domain, rollback.then_some(saved), &mut budget)
                .unwrap();
            assert!(budget.used() > 0);
            assert!(budget.used() <= guard);
            assert!(!budget.exhausted());
            let mut insufficient = WorkBudget::new(guard - 1);
            let used = insufficient.used();
            assert!(matches!(
                available(&insufficient, &insufficient.cancellation(), guard),
                Err(PrepareError::Admission(AdmissionStop::Insufficient))
            ));
            assert_eq!(insufficient.used(), used);
            assert!(insufficient.charge(1));
        }
    }
}

#[test]
fn ordinary_small_root_preparation_fits_the_default_work_budget() {
    for is_static in [false, true] {
        let (world, entry, config, facts) = fixture(Fork::Osaka, is_static);
        let original = transfer::initial(&world, &entry, config.domain().unwrap()).unwrap();
        let mut budget = WorkBudget::new(config.max_work);
        assert!(ready(&original, &world, &entry, &config, &facts, &mut budget).is_ok());
        assert!(!budget.exhausted());
        assert!(budget.used() < config.max_work);
    }
}
