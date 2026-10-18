use super::*;
use crate::{
    Fork,
    bytecode::Program,
    domain::{
        relational::RelationLimits,
        symbolic::{ExprError, ExprId, SlotStateId, SourceId, StateError},
    },
    world::{AbstractLog, Account, ByteArray, Code, Existence, LogKey, LogSourceKind, World},
};
use alloy_primitives::{B256, U256};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn input() -> Store {
    let mut world = World::new(Fork::Osaka, "checked unlisted preparation");
    let mut account = Account::from_hex("00", world.fork()).unwrap();
    account.storage_unknown = true;
    account.storage.insert(U256::from(8), word(5));
    world.insert(address(0x101), account).unwrap();
    let mut store = Store::new_checked(&world, Domain::default()).unwrap();
    store.may_touch_all();
    store
}

#[test]
fn every_explicit_map_and_both_log_address_roles_protect_the_original_owner() {
    let domain = Domain::default();
    let mut store = input();
    store.balances.insert(address(0x201), word(3));
    store.codes.insert(address(0x202), Code::Empty);
    store.nonces.insert(address(0x203), word(4));
    store.existence.insert(address(0x204), Existence::Present);
    store.created.insert(address(0x205), Some(false));
    store.pending_destruction.insert(address(0x206), Some(false));
    store.touched.insert(address(0x207), Some(false));
    store.persistent.defaults.insert(address(0x208), word(9));
    store.transient.defaults.insert(address(0x209), word(0));
    store
        .persistent
        .roots
        .insert(address(0x20a), SlotStateId::zero());
    store
        .transient
        .roots
        .insert(address(0x20b), SlotStateId::zero());
    store
        .persistent
        .slots
        .insert((address(0x20c), U256::from(8)), word(12));
    store
        .transient
        .slots
        .insert((address(0x20d), U256::from(8)), word(0));
    store
        .persistent
        .initial_independence
        .set_slot(address(0x201), U256::from(8), true);
    store.possible_logs.insert(
        LogKey {
            address: address(0x20e),
            code_address: address(0x20f),
            code_hash: B256::ZERO,
            source_kind: LogSourceKind::Runtime,
            pc: 7,
        },
        AbstractLog {
            topics: vec![word(19)],
            data: ByteArray::empty(),
        },
    );
    let protected = store.addresses();
    assert_eq!(protected.len(), 16);
    let roots: Vec<_> = protected
        .iter()
        .map(|owner| (*owner, store.slot_root(*owner, false).unwrap()))
        .collect();
    let before = store.clone();
    store
        .prepare_unlisted_finalization_checked(domain, &mut WorkBudget::new(20_000_000))
        .unwrap();
    for (owner, root) in roots {
        assert_eq!(store.slot_root(owner, false).unwrap(), root, "{owner}");
        assert_eq!(store.storage_default(owner), before.storage_default(owner));
        for key in [U256::from(8), U256::from(9)] {
            assert_eq!(
                store.persistent.initial_independence.at(owner, key),
                before.persistent.initial_independence.at(owner, key)
            );
        }
    }
    assert_eq!(store.persistent.slots, before.persistent.slots);
    assert_eq!(store.persistent.operands, before.persistent.operands);
    assert_ne!(store.global_storage_state(), before.global_storage_state());
    // No scalar/lifecycle/log/transient change is hidden by the plane update.
    store.persistent = before.persistent.clone();
    assert_eq!(store, before);
}

#[test]
fn checked_fresh_refusal_and_late_protected_root_refusal_are_atomic() {
    let domain = Domain::default();
    let limits = domain.spec().relations().expressions();
    let mut store = input();
    let protected = store.addresses();
    let before = store.persistent.clone();
    assert_eq!(
        store
            .persistent
            .havoc_except_with_fresh(&protected, limits, || None),
        Err(StoreError::State(StateError::IdentityExhausted))
    );
    assert_eq!(store.persistent, before);

    // The first protected owner fits. A later observed root does not, after
    // fresh-base construction, operand copying and an earlier staged insert.
    store.persistent.roots.insert(address(1), SlotStateId::zero());
    let protected = store.addresses();
    let before = store.persistent.clone();
    let small = crate::domain::symbolic::ExprLimits {
        max_nodes: 2,
        ..limits
    };
    assert!(matches!(
        store.persistent.havoc_except_checked(&protected, small),
        Err(StoreError::State(StateError::Limit { .. }))
    ));
    assert_eq!(store.persistent, before);
}

#[test]
fn typed_expression_and_state_refusals_leave_the_live_finalization_input_unchanged() {
    let mut store = input();
    let expression_error = StoreError::Expression(ExprError::Limit {
        nodes: 17,
        depth: 2,
    });
    store.persistent.failure = Some(expression_error.clone());
    let before = store.clone();
    assert_eq!(
        store.finalize_transaction_checked(
            Domain::default(),
            None,
            &mut WorkBudget::new(20_000_000),
        ),
        Err(FinalizationError::Store(expression_error.clone()))
    );
    assert_eq!(store, before);
    assert_eq!(
        store.finalize_transaction(Domain::default()),
        Err(expression_error)
    );
    assert_eq!(store, before);

    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 1,
        ..RelationLimits::default()
    }));
    let mut store = input();
    let before = store.clone();
    assert!(matches!(
        store.finalize_transaction_checked(domain, None, &mut WorkBudget::new(20_000_000)),
        Err(FinalizationError::Store(StoreError::State(
            StateError::Limit { .. }
        )))
    ));
    assert_eq!(store, before);
    assert!(store.finalize_transaction(domain).is_err());
    assert_eq!(store, before);
}

#[test]
fn cancellation_refuses_helper_and_both_finalizers_without_replacing_roots() {
    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut store = input();
        let before = store.clone();
        let protected = store.addresses();
        let limits = Domain::default().spec().relations().expressions();
        cancellation.cancel();
        assert_eq!(
            store.persistent.havoc_except_with_fresh(&protected, limits, || {
                panic!("cancelled preparation must not allocate a source")
            }),
            Err(StoreError::State(StateError::Cancelled))
        );
        assert_eq!(store, before);
        assert_eq!(
            store.finalize_transaction_checked(
                Domain::default(),
                None,
                &mut WorkBudget::new(20_000_000),
            ),
            Err(FinalizationError::Cancelled)
        );
        assert_eq!(store, before);
        assert_eq!(
            store.finalize_transaction(Domain::default()),
            Err(StoreError::State(StateError::Cancelled))
        );
        assert_eq!(store, before);
    });
}

#[test]
fn refusal_before_discovery_or_preparation_preserves_the_original_source() {
    let domain = Domain::default();
    let before = input();
    let discovery = PreparationWork::discovery(before.finalization_entries());
    let action = PreparationWork::measure(&before.persistent, &before.addresses()).total();
    for maximum in [0, discovery - 1, discovery, discovery + action - 1] {
        let mut store = before.clone();
        let mut budget = WorkBudget::new(maximum);
        assert_eq!(
            store.prepare_unlisted_finalization_checked(domain, &mut budget),
            Err(FinalizationError::Work),
            "maximum={maximum}"
        );
        assert_eq!(store, before);
        assert!(budget.exhausted());
        assert_eq!(
            budget.used(),
            if maximum < discovery { 0 } else { discovery }
        );
    }
    let mut store = before.clone();
    let mut budget = WorkBudget::new(discovery + action);
    store
        .prepare_unlisted_finalization_checked(domain, &mut budget)
        .unwrap();
    assert_eq!(budget.used(), discovery + action);
    assert_ne!(store.global_storage_state(), before.global_storage_state());

    let mut completed = before.clone();
    let mut full = WorkBudget::new(20_000_000);
    completed.finalize_transaction_checked(domain, None, &mut full).unwrap();
    let mut refused = before.clone();
    assert_eq!(
        refused.finalize_transaction_checked(domain, None, &mut WorkBudget::new(full.used() - 1)),
        Err(FinalizationError::Work)
    );
    assert_eq!(refused, before);
}

#[test]
fn every_missing_owner_prepays_scoping_the_entire_conditional_base() {
    let domain = Domain::default();
    let limits = domain.spec().relations().expressions();
    let mut store = input();
    store.persistent.roots.clear();
    store.persistent.base = SlotStateId::input(SourceId::fresh().unwrap())
        .join_guarded(
            &SlotStateId::input(SourceId::fresh().unwrap()),
            ExprId::fresh().unwrap(),
            limits.into(),
        )
        .unwrap();
    let one = PreparationWork::measure(&store.persistent, &store.addresses());
    assert_eq!(one.scoping, store.global_storage_state().nodes() * 16);
    store.created.insert(address(0x202), Some(false));
    let two = PreparationWork::measure(&store.persistent, &store.addresses());
    assert_eq!(two.scoping, one.scoping * 2);
    let roots: Vec<_> = store
        .addresses()
        .into_iter()
        .map(|owner| (owner, store.slot_root(owner, false).unwrap()))
        .collect();
    store.prepare_unlisted_finalization(domain).unwrap();
    for (owner, root) in roots {
        assert_eq!(store.slot_root(owner, false).unwrap(), root);
    }
}

#[test]
fn preparation_receipt_never_walks_or_prices_runtime_bytes_as_owner_work() {
    let mut store = input();
    let short = PreparationWork::measure(&store.persistent, &store.addresses());
    let discovery = PreparationWork::discovery(store.finalization_entries());
    store.codes.insert(
        address(0x101),
        Code::Runtime(Program::from_hex(&"00".repeat(4096)).unwrap()),
    );
    assert_eq!(
        PreparationWork::measure(&store.persistent, &store.addresses()),
        short
    );
    assert_eq!(
        PreparationWork::discovery(store.finalization_entries()),
        discovery
    );
    assert_eq!(PreparationWork::discovery(usize::MAX), usize::MAX);
    let saturated = PreparationWork {
        setup: 32,
        maps: usize::MAX,
        values: 2,
        scoping: 0,
    };
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(budget.charge(discovery));
    assert_eq!(
        super::reserve(&mut budget, saturated.total()),
        Err(FinalizationError::Work)
    );
    assert_eq!(budget.used(), discovery);
    assert!(budget.exhausted());
}
