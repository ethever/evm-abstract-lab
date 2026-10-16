//! Storage-order, baseline, and rollback regressions for the world wrapper.
use super::super::*;
use crate::{
    Fork,
    domain::{
        relational::{CheckResult, RelationLimits, RelationState, ValueQuery},
        symbolic::{ExprKind, ExprLimits, SlotStateKind},
    },
    world::Account,
};
use revm_bytecode::opcode;

fn v(n: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}
fn sym() -> AbstractValue {
    AbstractValue::top().with_expression(ExprId::fresh().unwrap())
}
fn world(complete: bool) -> (World, Address) {
    let address = Address::repeat_byte(0x31);
    let mut world = World::offline(Fork::Osaka, "symbolic slots", "leaf");
    world
        .insert(
            address,
            if complete {
                Account::empty()
            } else {
                Account::unknown()
            },
        )
        .unwrap();
    (world, address)
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        ..RelationLimits::default()
    }
}
fn assert_numeric(a: &AbstractValue, b: &AbstractValue) {
    assert_eq!(a.numeric(), b.numeric());
}
#[test]
fn unknown_same_key_latest_write_and_historical_read() {
    let (world, address) = world(false);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    let k = sym();
    let initial = store.read(address, &k, domain);
    assert_eq!(
        initial.expression(),
        store.read(address, &k, domain).expression()
    );
    store.write_checked(address, &k, &v(7), domain).unwrap();
    assert_eq!(
        store.read(address, &k, domain).singleton(),
        Some(U256::from(7))
    );
    let old = store.read(address, &k, domain);
    store.write_checked(address, &k, &v(9), domain).unwrap();
    assert_eq!(
        store.read(address, &k, domain).singleton(),
        Some(U256::from(9))
    );
    assert_eq!(old.singleton(), Some(U256::from(7)));
    assert!(initial.expression().is_some());
}
#[test]
fn mixed_concrete_symbolic_writes_keep_order_and_numeric_bounds() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    let k = sym();
    store.write_checked(address, &k, &v(7), domain).unwrap();
    store.write_checked(address, &v(0), &v(9), domain).unwrap();
    let value = store.read(address, &k, domain);
    assert!(value.contains(U256::from(7)));
    assert!(value.contains(U256::from(9)));
    assert!(!value.contains(U256::from(11)));
    let p = policy();
    let mut eq = RelationState::default();
    eq.observe(k.expression().unwrap(), v(0).numeric(), &p)
        .unwrap();
    assert_eq!(
        eq.local_value(value.expression().unwrap(), &p),
        ValueQuery::Exact(U256::from(9))
    );
    let mut ne = RelationState::default();
    ne.observe(k.expression().unwrap(), v(1).numeric(), &p)
        .unwrap();
    assert_eq!(
        ne.local_value(value.expression().unwrap(), &p),
        ValueQuery::Exact(U256::from(7))
    );
}
#[test]
fn fresh_bases_are_stable_and_accounts_planes_are_independent() {
    let (world, address) = world(false);
    let domain = Domain::default();
    let store = Store::new(&world);
    let other = Address::repeat_byte(0x32);
    assert_eq!(
        store.slot_root(address, false).unwrap(),
        Store::new(&world).slot_root(address, false).unwrap()
    );
    assert_ne!(
        store.slot_root(address, false).unwrap(),
        store.slot_root(other, false).unwrap()
    );
    assert!(matches!(
        store.slot_root(address, false).unwrap().kind(),
        SlotStateKind::Input { .. }
    ));
    assert_eq!(
        store.read_transient(address, &v(0), domain).singleton(),
        Some(U256::ZERO)
    );
    assert_ne!(
        store.read(address, &v(0), domain).expression(),
        store.read(other, &v(0), domain).expression()
    );
}
#[test]
fn observed_initial_slots_bind_symbolic_and_numeric_facts() {
    let (mut world, address) = world(false);
    let domain = Domain::default();
    let token = ExprId::fresh().unwrap();
    let observed = AbstractValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(token);
    world
        .accounts
        .get_mut(&address)
        .unwrap()
        .storage
        .insert(U256::from(4), observed.clone());
    let store = Store::new(&world);
    assert_numeric(&store.read(address, &v(4), domain), &observed);
    let read = store.read(address, &v(4), domain);
    let p = policy();
    let invalid = ExprId::operation(
        opcode::EQ,
        &[
            read.expression().unwrap().clone(),
            ExprId::constant(U256::from(9)),
        ],
        p.expressions(),
    )
    .unwrap();
    assert_eq!(
        RelationState::default().assume(&invalid, true, &p),
        CheckResult::ProvenImpossible
    );
    assert!(store.read(address, &v(5), domain).numeric().is_top());
    assert!(matches!(
        store
            .read(address, &v(5), domain)
            .expression()
            .unwrap()
            .kind(),
        ExprKind::SlotRead { .. }
    ));
}
#[test]
fn baseline_identity_survives_new_rpc_observations() {
    let (mut world, address) = world(false);
    let before = Store::new(&world);
    let key = v(1);
    let domain = Domain::default();
    let source = if let SlotStateKind::Input { source, .. } =
        before.slot_root(address, false).unwrap().kind()
    {
        source.clone()
    } else {
        panic!("unknown persistent base")
    };
    world
        .install_storage(address, BTreeMap::from([(U256::from(2), v(11))]))
        .unwrap();
    let after = Store::new(&world);
    if let SlotStateKind::Input {
        source: next,
        bindings,
    } = after.slot_root(address, false).unwrap().kind()
    {
        assert_eq!(next, &source);
        assert_eq!(bindings.len(), 1);
    } else {
        panic!("observed base")
    }
    assert!(after.read(address, &key, domain).numeric().is_top());
    assert_eq!(
        after.missing_initial_slots(&world, address, &v(1)),
        vec![U256::from(1)]
    );
}
#[test]
fn persistent_transient_snapshot_and_creation_reset() {
    let (world, address) = world(false);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    let k = sym();
    store.write_checked(address, &k, &v(7), domain).unwrap();
    store
        .write_transient_checked(address, &k, &v(9), domain)
        .unwrap();
    let before = store.clone();
    let snapshot = store.snapshot();
    store.begin_creation(address);
    assert_eq!(
        store.read(address, &k, domain).singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        store.read_transient(address, &k, domain).singleton(),
        Some(U256::ZERO)
    );
    store.restore(snapshot);
    assert_eq!(store, before);
    store.finalize_transaction(domain);
    assert_eq!(
        store.read(address, &k, domain).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        store.read_transient(address, &k, domain).singleton(),
        Some(U256::ZERO)
    );
}
#[test]
fn guarded_store_join_retains_same_selector_as_key() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut left = Store::new(&world);
    let mut right = left.clone();
    left.write_checked(address, &v(0), &v(7), domain).unwrap();
    right.write_checked(address, &v(1), &v(7), domain).unwrap();
    let guard = ExprId::fresh().unwrap();
    let joined = left
        .join_guarded(&right, domain, guard.clone(), ExprLimits::default())
        .unwrap();
    let key = v(0)
        .join_guarded(&v(1), domain, guard.clone(), ExprLimits::default())
        .unwrap();
    let read = joined.read(address, &key, domain);
    assert!(read.contains(U256::from(7)));
    // An unknown selector is not enumerated. Independent local joins lose the
    // key/state correlation, while each explicitly selected branch stays exact.
    let ValueQuery::Bounds(bounds) =
        RelationState::default().local_value(read.expression().unwrap(), &policy())
    else {
        panic!("expected the documented key/state correlation precision gap");
    };
    assert!(bounds.contains(U256::from(7)));
    assert!(
        bounds.contains(U256::ZERO),
        "local bounds retain a spurious zero"
    );
    for nonzero in [false, true] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&guard, nonzero, &policy()),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            relations.local_value(read.expression().unwrap(), &policy()),
            ValueQuery::Exact(U256::from(7))
        );
    }
}
#[test]
fn full_source_fresh_rename_and_rollback_preserve_old_reads() {
    let (world, address) = world(false);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store.havoc_account(address);
    let old = store.read(address, &v(0), domain);
    let snapshot = store.snapshot();
    let map = store
        .fresh_leaves()
        .into_iter()
        .map(|e| (e, ExprId::fresh().unwrap()))
        .collect::<BTreeMap<_, _>>();
    assert!(map.len() >= 2);
    store.rename_fresh(&map, ExprLimits::default()).unwrap();
    assert_ne!(
        store.read(address, &v(0), domain).expression(),
        old.expression()
    );
    store.restore(snapshot);
    assert_eq!(
        store.read(address, &v(0), domain).expression(),
        old.expression()
    );
}
#[test]
fn stable_same_key_loop_and_self_join_are_idempotent() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    let key = sym();
    store.write_checked(address, &key, &v(7), domain).unwrap();
    let old = store.clone();
    for _ in 0..20 {
        store.write_checked(address, &key, &v(7), domain).unwrap();
    }
    assert_eq!(store, old);
    assert_eq!(store.join(&store, domain), store);
}
#[test]
fn same_root_different_operand_facts_join_soundly() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let expr = ExprId::fresh().unwrap();
    let a = AbstractValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(expr.clone());
    let b = AbstractValue::unsigned_range(U256::from(9), U256::from(10))
        .unwrap()
        .with_expression(expr);
    let mut left = Store::new(&world);
    let mut right = left.clone();
    left.write_checked(address, &v(0), &a, domain).unwrap();
    right.write_checked(address, &v(0), &b, domain).unwrap();
    assert_eq!(
        left.slot_root(address, false).unwrap(),
        right.slot_root(address, false).unwrap()
    );
    let joined = left.join(&right, domain);
    for n in 7..=10 {
        assert!(joined.read(address, &v(0), domain).contains(U256::from(n)));
    }
}
#[test]
fn checked_failure_keeps_transaction_snapshot() {
    let (world, address) = world(true);
    let limits = crate::domain::relational::RelationLimits {
        max_nodes: 1,
        max_depth: 1,
        ..RelationLimits::default()
    };
    let domain = Domain::from_spec(Domain::default().spec().with_relations(limits));
    let mut store = Store::new(&world);
    let old = store.clone();
    assert!(store.write_checked(address, &sym(), &v(7), domain).is_err());
    assert_eq!(store, old);
}

#[test]
fn independent_offline_worlds_same_label_have_distinct_bases() {
    let address = Address::repeat_byte(0x77);
    let a = World::new(Fork::Osaka, "same descriptive label");
    let b = World::new(Fork::Osaka, "same descriptive label");
    assert_ne!(
        Store::new(&a).slot_root(address, false).unwrap(),
        Store::new(&b).slot_root(address, false).unwrap()
    );
    assert_eq!(
        Store::new(&a).slot_root(address, false).unwrap(),
        Store::new(&a.clone()).slot_root(address, false).unwrap()
    );
    assert!(
        serde_json::to_value(&a)
            .unwrap()
            .get("storage_source")
            .is_some()
    );
}
#[test]
fn joined_default_base_read_cancellation_is_typed() {
    let (world, address) = world(false);
    let domain = Domain::default();
    let a = Store::new(&world);
    let mut b = a.clone();
    b.havoc_all();
    let joined = a
        .join_guarded(&b, domain, ExprId::fresh().unwrap(), ExprLimits::default())
        .unwrap();
    let cancel = analysis_control::Cancellation::new();
    cancel.cancel();
    analysis_control::with_cancellation(&cancel, || {
        let result = joined.read_checked(Address::repeat_byte(0x99), &v(0), domain);
        assert!(matches!(
            result,
            Err(StoreError::State(StateError::Cancelled))
                | Err(StoreError::Expression(ExprError::Cancelled))
        ));
    });
    assert!(joined.read_checked(address, &v(0), domain).is_ok());
}
#[test]
fn ordinary_join_unites_numeric_key_facts_at_shared_root() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let token = ExprId::fresh().unwrap();
    let ka = AbstractValue::unsigned_range(U256::from(1), U256::from(2))
        .unwrap()
        .with_expression(token.clone());
    let kb = AbstractValue::unsigned_range(U256::from(3), U256::from(4))
        .unwrap()
        .with_expression(token);
    let mut a = Store::new(&world);
    let mut b = a.clone();
    a.write_checked(address, &ka, &v(7), domain).unwrap();
    b.write_checked(address, &kb, &v(7), domain).unwrap();
    let joined = a.join(&b, domain);
    for key in 1..=4 {
        assert!(
            joined
                .read(address, &v(key), domain)
                .contains(U256::from(7))
        );
    }
}

#[test]
fn complete_symbolic_observation_has_local_bound_and_zero_unobserved_base() {
    let (mut world, address) = world(true);
    let observed = AbstractValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    world
        .accounts
        .get_mut(&address)
        .unwrap()
        .storage
        .insert(U256::from(4), observed);
    let store = Store::new(&world);
    let value = store.read(address, &v(4), Domain::default());
    let p = policy();
    let eq = ExprId::operation(
        opcode::EQ,
        &[
            value.expression().unwrap().clone(),
            ExprId::constant(U256::from(9)),
        ],
        p.expressions(),
    )
    .unwrap();
    assert_eq!(
        RelationState::default().assume(&eq, true, &p),
        CheckResult::ProvenImpossible
    );
    assert_eq!(
        store.read(address, &v(5), Domain::default()).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn checked_store_self_join_respects_budget_cancel_and_failure() {
    let (world, address) = world(true);
    let store = Store::new(&world);
    let guard = ExprId::constant(U256::ZERO);
    assert!(
        store
            .join_guarded(
                &store,
                Domain::default(),
                guard.clone(),
                ExprLimits {
                    max_nodes: 0,
                    max_depth: 0
                }
            )
            .is_err()
    );
    let cancel = analysis_control::Cancellation::new();
    cancel.cancel();
    analysis_control::with_cancellation(&cancel, || {
        assert!(matches!(
            store.join_guarded(
                &store,
                Domain::default(),
                guard.clone(),
                ExprLimits::default()
            ),
            Err(StoreError::State(StateError::Cancelled))
        ));
    });
    let mut failed = store.clone();
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 1,
        max_depth: 1,
        ..RelationLimits::default()
    }));
    failed.write(address, &sym(), &v(7), domain);
    assert!(
        failed
            .join_guarded(&failed, Domain::default(), guard, ExprLimits::default())
            .is_err()
    );
}

#[test]
fn ordinary_store_union_retains_an_alternative_under_old_guard_false() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut a = Store::new(&world);
    let mut b = a.clone();
    a.write_checked(address, &v(0), &v(7), domain).unwrap();
    b.write_checked(address, &v(0), &v(9), domain).unwrap();
    let guard = ExprId::fresh().unwrap();
    let paired = a
        .join_guarded(&b, domain, guard.clone(), ExprLimits::default())
        .unwrap();
    let union = paired.join(&a, domain);
    let value = union.read(address, &v(0), domain);
    let p = policy();
    let mut relations = RelationState::default();
    relations.observe(&guard, v(0).numeric(), &p).unwrap();
    let ValueQuery::Bounds(bounds) = relations.local_value(value.expression().unwrap(), &p) else {
        panic!("ordinary union must retain both alternatives under the old false guard");
    };
    for candidate in [7, 9] {
        assert!(value.contains(U256::from(candidate)));
        assert!(bounds.contains(U256::from(candidate)));
    }
    let eq = ExprId::operation(
        opcode::EQ,
        &[
            value.expression().unwrap().clone(),
            ExprId::constant(U256::from(7)),
        ],
        p.expressions(),
    )
    .unwrap();
    assert_eq!(relations.assume(&eq, true, &p), CheckResult::MayBeFeasible);
}

#[test]
fn numeric_only_slot_aliases_constrain_native_reads() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let key = domain.join(&v(0), &v(1));
    let mut store = Store::new(&world);
    store.write_checked(address, &key, &v(7), domain).unwrap();
    let read = store.read(address, &v(2), domain);
    assert_eq!(read.singleton(), Some(U256::ZERO));
    // The scalar read excludes slot 2, but local alias comparison does not
    // exclude it from a nonsingleton synthesized key solely by its finite set.
    let ValueQuery::Bounds(bounds) =
        RelationState::default().local_value(read.expression().unwrap(), &policy())
    else {
        panic!("expected the documented finite-key alias precision gap");
    };
    assert!(bounds.contains(U256::ZERO));
    assert!(
        bounds.contains(U256::from(7)),
        "local bounds retain the spurious alias"
    );
    let root = store.slot_root(address, false).unwrap();
    let SlotStateKind::Store { key: stored, .. } = root.kind() else {
        panic!("expected the original persistent write");
    };
    for candidate in [0, 1] {
        let mut relations = RelationState::default();
        relations
            .observe(stored, v(candidate).numeric(), &policy())
            .unwrap();
        assert_eq!(
            relations.local_value(read.expression().unwrap(), &policy()),
            ValueQuery::Exact(U256::ZERO)
        );
    }
}

#[test]
fn initialization_uses_the_callers_state_policy_and_reports_failure() {
    let (mut world, address) = world(true);
    for i in 0..65u64 {
        world
            .accounts
            .get_mut(&address)
            .unwrap()
            .storage
            .insert(U256::from(i), v(i + 1));
    }
    let default = Domain::default();
    assert!(matches!(
        Store::new_checked(&world, default),
        Err(StoreError::State(StateError::Limit { .. }))
    ));
    assert!(Store::new(&world).symbolic_limit_reached());
    let bigger = Domain::from_spec(default.spec().with_relations(RelationLimits {
        max_depth: 128,
        ..RelationLimits::default()
    }));
    let store = Store::new_checked(&world, bigger).unwrap();
    assert!(!store.symbolic_limit_reached());
    assert_eq!(
        store.read(address, &v(64), bigger).singleton(),
        Some(U256::from(65))
    );
}
