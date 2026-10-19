//! Unknown-world capability, whole-payload coverage and fresh replay regressions.

use super::super::*;
use crate::{
    Fork,
    domain::{
        relational::{CheckResult, RelationLimits, RelationState, ValueQuery},
        symbolic::ExprLimits,
    },
    world::Account,
};
use revm_bytecode::opcode;

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

#[test]
fn unlisted_lifecycle_owners_keep_the_same_runtime_and_summary_union() {
    let (world, _, _) = fixture();
    let domain = Domain::default();
    let owner = Address::repeat_byte(0xee);
    let unlisted = Address::repeat_byte(0xef);
    let mut left = Store::new(&world);
    left.created_default = None;
    left.created.insert(owner, Some(true));
    left.pending_destruction.insert(owner, Some(true));
    let mut right = Store::new(&world);
    right.created_default = Some(true);
    right.pending_destruction_default = None;
    right.pending_destruction.insert(owner, Some(true));
    let first = left.summary_projection(domain).unwrap();
    let second = right.summary_projection(domain).unwrap();
    for (left, right) in [(&left, &right), (&right, &left)] {
        let runtime = left
            .join_guarded(
                right,
                domain,
                ExprId::fresh().unwrap(),
                ExprLimits::default(),
            )
            .unwrap();
        let summary = left.summary_projection(domain).unwrap().join(
            &right.summary_projection(domain).unwrap(),
            domain,
            false,
        );
        let replay = summary.instantiate(domain).unwrap();
        assert!(summary.covers(&first, domain) && summary.covers(&second, domain));
        for state in [&runtime, &replay] {
            assert_eq!(state.created_in_transaction(owner), Some(true));
            assert_eq!(state.pending_destruction(owner), Some(true));
            assert_eq!(state.created_in_transaction(unlisted), None);
            assert_eq!(state.pending_destruction(unlisted), None);
        }
        assert_eq!(runtime.summary_projection(domain).unwrap(), summary);
    }
}

#[test]
fn numerical_projection_compresses_storage_history_under_a_smaller_replay_limit() {
    let (world, a, _) = fixture();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    for value in 1..=20 {
        store
            .write_checked(a, &word(value % 2), &word(value), domain)
            .unwrap();
    }
    let smaller = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 48,
        ..domain.spec().relations()
    }));
    assert!(store.slot_root(a, false).unwrap().nodes() > 48);
    assert!(store.read_checked(a, &word(0), smaller).is_err());
    let summary = store.summary_projection(smaller).unwrap();
    let replay = summary.instantiate(smaller).unwrap();
    assert!(replay.slot_root(a, false).unwrap().nodes() <= 48);
    assert_eq!(
        replay.read(a, &word(0), smaller).singleton(),
        Some(U256::from(20))
    );
    assert_eq!(
        replay.read(a, &word(1), smaller).singleton(),
        Some(U256::from(19))
    );
    assert_eq!(
        replay.read(a, &word(2), smaller).singleton(),
        Some(U256::ZERO)
    );
    assert!(summary.work_size() >= replay.work_size());
    let failure = StoreError::Expression(ExprError::Limit {
        nodes: 1001,
        depth: 1,
    });
    store.persistent.failure = Some(failure.clone());
    assert_eq!(store.summary_projection(smaller), Err(failure));
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            replay.summary_projection(smaller),
            Err(StoreError::Expression(ExprError::Cancelled))
        );
    });
}

fn fixture() -> (World, Address, Address) {
    let a = Address::repeat_byte(0xa1);
    let c = Address::repeat_byte(0xc1);
    let mut world = World::new(Fork::Osaka, "unknown-world store summaries");
    for owner in [a, c] {
        let mut account = Account::from_hex("00", Fork::Osaka).unwrap();
        account.storage_unknown = false;
        account.storage.insert(U256::ZERO, word(5));
        account.nonce = word(1);
        account.balance = word(0);
        world.insert(owner, account).unwrap();
    }
    (world, a, c)
}

#[test]
fn ordinary_unknown_steps_protect_full_known_storage_and_cover_omitted_world() {
    let (world, a, c) = fixture();
    let domain = Domain::default();
    let other = Address::repeat_byte(0xb1);
    let mut store = Store::new(&world);
    store.write_transient(a, &word(4), &word(9), domain);
    let symbolic = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    store.write_checked(c, &symbolic, &word(7), domain).unwrap();
    let before = store.clone();
    let checkpoint = store.snapshot();
    store
        .unknown_effects(&BTreeSet::from([a, c]), None, false, domain)
        .unwrap();
    for owner in [a, c] {
        assert_eq!(
            store.slot_root(owner, false),
            before.slot_root(owner, false)
        );
        assert_eq!(store.slot_root(owner, true), before.slot_root(owner, true));
        assert_eq!(store.code(owner), before.code(owner));
        assert_eq!(store.nonce(owner), before.nonce(owner));
        assert_eq!(store.created_in_transaction(owner), Some(false));
        assert_eq!(store.pending_destruction(owner), Some(false));
    }
    assert_eq!(
        store.read(c, &symbolic, domain).singleton(),
        Some(U256::from(7))
    );
    assert!(store.read(other, &word(0), domain).numeric().is_top());
    assert!(
        store
            .read_transient(other, &word(0), domain)
            .numeric()
            .is_top()
    );
    assert!(store.read_balance(a).numeric().is_top());
    assert!(store.read_balance(other).numeric().is_top());
    assert_eq!(store.created_in_transaction(other), None);
    assert_eq!(store.pending_destruction(other), None);
    assert!(store.logs_unknown());
    assert!(
        store
            .missing_initial_slots(&world, other, &word(0))
            .is_empty()
    );
    store.restore(checkpoint);
    let mut reverted = before;
    reverted.may_touch_account(Address::from_word(U256::from(3).into()));
    assert_eq!(store, reverted);
    assert_eq!(store.created_in_transaction(other), Some(false));
}

#[test]
fn storage_capability_and_static_effect_preserve_values_and_track_possible_touches() {
    let (world, a, _) = fixture();
    let mut store = Store::new(&world);
    let before = store.clone();
    store.havoc_storage_owner(a);
    assert!(
        store
            .read(a, &word(0), Domain::default())
            .numeric()
            .is_top()
    );
    assert_eq!(store.code(a), before.code(a));
    assert_eq!(store.nonce(a), before.nonce(a));
    assert_eq!(store.read_balance(a), before.read_balance(a));
    assert_eq!(store.created_in_transaction(a), Some(false));
    assert!(!store.logs_unknown());
    let mut expected = store.clone();
    expected.may_touch_all();
    store
        .apply_unknown_effects(&BTreeSet::new(), Some(a), true, Fork::Osaka)
        .unwrap();
    assert_eq!(store, expected);
}

#[test]
fn delegate_nonce_and_new_owner_deletion_are_covered_at_transaction_end() {
    let (world, a, c) = fixture();
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut store = Store::new(&world);
        store.begin_creation(a);
        store.deploy_code(a, Program::from_hex_with_fork("00", fork).unwrap());
        let code = store.code(a).cloned();
        store
            .apply_unknown_effects(&BTreeSet::from([a, c]), Some(a), false, fork)
            .unwrap();
        assert!(
            store
                .read(a, &word(0), Domain::default())
                .numeric()
                .is_top()
        );
        assert!(store.nonce(a).numeric().is_top());
        assert_eq!(store.pending_destruction(a), None);
        assert_eq!(store.created_in_transaction(a), Some(true));
        assert_eq!(store.code(a), code.as_ref());
        assert_eq!(
            store.read(c, &word(0), Domain::default()).singleton(),
            Some(U256::from(5))
        );
        store.finalize_transaction(Domain::default()).unwrap();
        assert_eq!(store.code(a), Some(&Code::Unknown));
        assert_eq!(store.existence(a), Existence::Unknown);
        assert_eq!(store.created_in_transaction(a), Some(false));
        assert_eq!(store.pending_destruction(a), Some(false));
        assert_eq!(
            store
                .read_transient(a, &word(0), Domain::default())
                .singleton(),
            Some(U256::ZERO)
        );
    }
}

#[test]
fn empty_candidate_is_not_frozen_against_unknown_deployment() {
    let (mut world, a, _) = fixture();
    let empty = Address::repeat_byte(0xe1);
    world.insert(empty, Account::absent()).unwrap();
    let mut store = Store::new(&world);
    store
        .unknown_effects(&BTreeSet::from([a, empty]), None, false, Domain::default())
        .unwrap();
    assert!(store.code(empty).is_none_or(|code| *code == Code::Unknown));
    assert_eq!(store.existence(empty), Existence::Unknown);
    assert_eq!(store.created_in_transaction(empty), None);
    assert!(
        store
            .read(empty, &word(0), Domain::default())
            .numeric()
            .is_top()
    );
}

#[test]
fn canonical_join_contains_finite_iterations_and_stabilizes_despite_fresh_sources() {
    let (world, a, c) = fixture();
    let domain = Domain::default();
    let mut input = Store::new(&world);
    input.write(a, &word(0), &word(0), domain);
    input.write(a, &word(1), &word(1), domain);
    input
        .unknown_effects(&BTreeSet::from([a, c]), None, false, domain)
        .unwrap();
    let initial = input.summary_projection(domain).unwrap();
    let mut summary = initial.clone();
    for value in 0..=2 {
        let mut incoming = summary.instantiate(domain).unwrap();
        incoming.write(a, &word(0), &word(value), domain);
        let incoming = incoming.summary_projection(domain).unwrap();
        let joined = summary.join(&incoming, domain, false);
        assert!(joined.covers(&summary, domain));
        assert!(joined.covers(&incoming, domain));
        assert_eq!(joined.join(&joined, domain, false), joined);
        summary = joined;
    }
    assert!(summary.covers(&initial, domain));
    let stable = summary.clone();
    for _ in 0..12 {
        let incoming = summary
            .instantiate(domain)
            .unwrap()
            .summary_projection(domain)
            .unwrap();
        summary = summary.join(&incoming, domain, true);
        assert_eq!(summary, stable);
    }
    let replay = summary.instantiate(domain).unwrap();
    let x = replay.read(a, &word(0), domain);
    assert_eq!(
        x.constants().unwrap(),
        &BTreeSet::from([U256::ZERO, U256::from(1), U256::from(2)])
    );
    assert_eq!(
        replay.read(a, &word(1), domain).singleton(),
        Some(U256::from(1))
    );
    assert_eq!(
        replay.read(a, &word(99), domain).singleton(),
        Some(U256::ZERO)
    );
    assert!(replay.global_storage_default().numeric().is_top());
}

#[test]
fn projection_uses_roots_and_operands_instead_of_cache_and_covers_unknown_aliases() {
    let (world, a, _) = fixture();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store.write(a, &word(4), &word(9), domain);
    store.persistent.slots.remove(&(a, U256::from(4)));
    let projected = store
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(
        projected.read(a, &word(4), domain).singleton(),
        Some(U256::from(9))
    );
    assert_eq!(
        projected.read(a, &word(99), domain).singleton(),
        Some(U256::ZERO)
    );
    let key = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    store.write(a, &key, &word(7), domain);
    let projected = store
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert!(projected.storage_default(a).numeric().is_top());
    assert!(projected.read(a, &word(4), domain).contains(U256::from(7)));
    assert!(projected.read(a, &word(4), domain).contains(U256::from(9)));
}

#[test]
fn complete_coverage_checks_defaults_lifecycle_code_balance_nonce_transient_and_logs() {
    let (world, a, _) = fixture();
    let domain = Domain::default();
    let base = Store::new(&world);
    let initial = base.summary_projection(domain).unwrap();
    let mut variants = Vec::new();
    let mut value = base.clone();
    value.persistent.base = SlotStateId::zero();
    value.persistent.global_default = word(0);
    // Reverse direction checks that the broader global default covers this state.
    let narrower = value.summary_projection(domain).unwrap();
    assert!(initial.covers(&narrower, domain));
    assert!(!narrower.covers(&initial, domain));
    let mut value = base.clone();
    value.transient.havoc_all();
    variants.push(value);
    let mut value = base.clone();
    value.write_nonce(a, word(3));
    variants.push(value);
    let mut value = base.clone();
    value.write_balance(a, word(3));
    variants.push(value);
    let mut value = base.clone();
    value.codes.insert(a, Code::Empty);
    variants.push(value);
    let mut value = base.clone();
    value.existence.insert(a, Existence::Absent);
    variants.push(value);
    let mut value = base.clone();
    value.created_default = None;
    variants.push(value);
    let mut value = base.clone();
    value.pending_destruction_default = None;
    variants.push(value);
    let mut value = base.clone();
    value.touch_account(a);
    variants.push(value);
    let mut value = base.clone();
    value.touched_default = None;
    variants.push(value);
    let mut value = base.clone();
    value
        .emit_log(
            LogKey {
                address: a,
                code_address: a,
                code_hash: alloy_primitives::B256::ZERO,
                source_kind: crate::world::LogSourceKind::Runtime,
                pc: 0,
            },
            AbstractLog {
                topics: vec![word(9)],
                data: ByteArray::exact(&[1, 2]),
            },
            domain,
        )
        .unwrap();
    variants.push(value);
    for value in variants {
        let incoming = value.summary_projection(domain).unwrap();
        assert!(!initial.covers(&incoming, domain));
        let joined = initial.join(&incoming, domain, false);
        assert!(joined.covers(&initial, domain));
        assert!(joined.covers(&incoming, domain));
    }
}

#[test]
fn projected_overwrites_keep_rpc_dependencies_and_historical_reads_separate() {
    let (mut world, a, _) = fixture();
    let domain = Domain::default();
    world.accounts.get_mut(&a).unwrap().storage_unknown = true;
    let mut store = Store::new(&world);
    let historical = store.read(a, &word(0), domain);
    store.write(a, &word(0), &word(9), domain);
    let summary = store.summary_projection(domain).unwrap();
    let replay = summary.instantiate(domain).unwrap();
    assert_eq!(historical.singleton(), Some(U256::from(5)));
    assert_eq!(
        replay.read(a, &word(0), domain).singleton(),
        Some(U256::from(9))
    );
    assert!(replay.missing_initial_slots(&world, a, &word(0)).is_empty());
    assert_eq!(
        replay.missing_initial_slots(&world, a, &word(99)),
        vec![U256::from(99)]
    );
    let second = summary.instantiate(domain).unwrap();
    assert_ne!(
        replay.slot_root(a, false).unwrap(),
        second.slot_root(a, false).unwrap()
    );
    assert_ne!(
        replay.read(a, &word(99), domain).expression(),
        second.read(a, &word(99), domain).expression()
    );
    assert!(replay.balance_default().expression().is_none());
}

#[test]
fn instantiated_bounded_cells_are_local_constraints_with_independent_slot_values() {
    let (world, a, _) = fixture();
    let domain = Domain::default();
    let bounds = AbstractValue::unsigned_range(U256::from(7), U256::from(8)).unwrap();
    let mut store = Store::new(&world);
    store.write(a, &word(2), &bounds, domain);
    store.write(a, &word(3), &bounds, domain);
    let replay = store
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    let first = replay.read(a, &word(2), domain);
    let second = replay.read(a, &word(3), domain);
    let policy = RelationLimits::default();
    for read in [&first, &second] {
        let ValueQuery::Bounds(bounds) =
            RelationState::default().local_value(read.expression().unwrap(), &policy)
        else {
            panic!("both concrete slot candidates must survive projection and fresh replay");
        };
        for candidate in [7, 8] {
            assert!(read.contains(U256::from(candidate)));
            assert!(bounds.contains(U256::from(candidate)));
        }
    }
    let equal = ExprId::operation(
        opcode::EQ,
        &[
            first.expression().unwrap().clone(),
            second.expression().unwrap().clone(),
        ],
        policy.expressions(),
    )
    .unwrap();
    assert_eq!(
        RelationState::default().assume(&equal, false, &policy),
        CheckResult::MayBeFeasible
    );
    for (left, right) in [(7, 8), (8, 7)] {
        let mut independent = RelationState::default();
        for (read, expected) in [(&first, left), (&second, right)] {
            independent
                .observe(
                    read.expression().unwrap(),
                    word(expected).numeric(),
                    &policy,
                )
                .unwrap();
        }
        assert_eq!(independent.check(&policy), CheckResult::MayBeFeasible);
        for (read, expected) in [(&first, left), (&second, right)] {
            assert_eq!(
                independent.local_value(read.expression().unwrap(), &policy),
                ValueQuery::Exact(U256::from(expected))
            );
        }
    }
    let invalid = ExprId::operation(
        opcode::EQ,
        &[
            first.expression().unwrap().clone(),
            ExprId::constant(U256::from(9)),
        ],
        ExprLimits::default(),
    )
    .unwrap();
    assert_eq!(
        RelationState::default().assume(&invalid, true, &policy),
        CheckResult::ProvenImpossible
    );
}

#[test]
fn canonical_work_covers_rebuilt_roots_scalars_and_event_payloads() {
    let (mut world, a, c) = fixture();
    world.accounts.get_mut(&a).unwrap().storage_unknown = true;
    let domain = Domain::default();
    let bounds = AbstractValue::unsigned_range(U256::from(7), U256::from(8)).unwrap();
    let mut store = Store::new(&world);
    for owner in [a, c] {
        store.write(owner, &word(2), &bounds, domain);
        store.write_transient(owner, &word(3), &bounds, domain);
        store.write_nonce(owner, bounds.clone());
        store.write_balance(owner, bounds.clone());
    }
    store
        .emit_log(
            LogKey {
                address: a,
                code_address: a,
                code_hash: alloy_primitives::B256::ZERO,
                source_kind: crate::world::LogSourceKind::Runtime,
                pc: 1,
            },
            AbstractLog {
                topics: vec![bounds],
                data: ByteArray::exact(&[1, 0, 2]),
            },
            domain,
        )
        .unwrap();
    let summary = store.summary_projection(domain).unwrap();
    let replay = summary.instantiate(domain).unwrap();
    assert!(
        summary.work_size() >= replay.work_size(),
        "summary={}, replay={}",
        summary.work_size(),
        replay.work_size()
    );
}

#[test]
fn static_touch_summary_preserves_data_roots_and_existing_must_touches() {
    let (world, a, b) = fixture();
    let domain = Domain::default();
    let mut input = Store::new_checked(&world, domain).unwrap();
    input.write_checked(a, &word(0), &word(42), domain).unwrap();
    input.touch_account(a);
    let root = input.slot_root(a, false).unwrap();
    let mut generic = Store::new_checked(&world, domain).unwrap();
    generic.may_touch_all();
    generic.touch_account(b);
    let summary = generic.summary_projection(domain).unwrap();
    let applied = summary.apply_static_touches(&input);
    let mut expected = input.clone();
    expected.may_touch_all();
    expected.touch_account(b);
    assert_eq!(applied, expected);
    assert_eq!(applied.slot_root(a, false).unwrap(), root);
    assert_eq!(
        applied.read(a, &word(0), domain).singleton(),
        Some(U256::from(42))
    );
    assert_eq!(applied.touched_in_transaction(a), Some(true));
    assert_eq!(applied.touched_in_transaction(b), Some(true));
}
