//! RPC discovery requests initial facts only while a path still needs them.

use crate::world::test_support::TestStore;
use std::num::NonZeroUsize;
use {
    crate::Address, crate::Fork, crate::U256, crate::domain::symbolic::ExprId,
    crate::world::test_support::TestAccount, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestValue, crate::world::test_support::TestWorld,
};

// Numeric, provenance and persistent identity assertions remain unchanged;
// reads now additionally carry their immutable state dependency.
macro_rules! assert_slot_eq {
    ($actual:expr,$expected:expr $(,)?) => {{
        let actual = $actual;
        let mut plain = actual.clone();
        plain.forget_expression();
        assert_eq!(plain, $expected);
        assert!(!actual.symbolic_limit_reached());
        if actual.singleton().is_none() {
            assert!(actual.expression().is_some());
        }
    }};
}

fn word(value: u64) -> TestValue {
    TestValue::constant(U256::from(value))
}

fn partial_world() -> (TestWorld, Address) {
    let address = Address::repeat_byte(0x31);
    let mut account = TestAccount::unknown();
    account.storage.insert(U256::from(4), word(11));
    let mut world = TestWorld::new(Fork::Osaka, "initial storage dependencies");
    world.insert(address, account).unwrap();
    (world, address)
}

fn keys(left: u64, right: u64) -> TestValue {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .join(&word(left), &word(right))
}

#[test]
fn missing_requests_require_finite_keys_and_incomplete_initial_coverage() {
    let (mut world, address) = partial_world();
    let store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(4, 5)),
        vec![U256::from(5)]
    );
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(2, 1)),
        vec![U256::from(1), U256::from(2)]
    );
    assert!(
        store
            .missing_initial_slots(&world, address, &TestValue::top())
            .is_empty()
    );
    assert!(
        store
            .missing_initial_slots(&world, address, &TestValue::unknown_byte())
            .is_empty()
    );
    let single = TestValue::unsigned_range(U256::from(5), U256::from(5)).unwrap();
    assert_eq!(
        store.missing_initial_slots(&world, address, &single),
        vec![U256::from(5)]
    );
    let absent = Address::repeat_byte(0x32);
    let complete = Address::repeat_byte(0x33);
    let omitted = Address::repeat_byte(0x34);
    world.insert(absent, TestAccount::absent()).unwrap();
    world.insert(complete, TestAccount::empty()).unwrap();
    let store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    for address in [absent, complete] {
        assert!(
            store
                .missing_initial_slots(&world, address, &word(5))
                .is_empty()
        );
    }
    assert_eq!(
        store.missing_initial_slots(&world, omitted, &word(5)),
        vec![U256::from(5)]
    );
}

#[test]
fn strong_writes_discard_initial_dependency_even_when_the_new_value_is_top() {
    let (world, address) = partial_world();
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.write(
        address,
        &word(1),
        &TestValue::top(),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    assert!(
        store
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_slot_eq!(
        store.read(
            address,
            &word(1),
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        TestValue::top()
    );
}

#[test]
fn finite_weak_writes_preserve_baseline_but_do_not_restore_discarded_dependencies() {
    let (world, address) = partial_world();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.write(address, &word(1), &word(3), domain.borrowed());
    store.write(address, &keys(1, 2), &word(7), domain.borrowed());
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_slot_eq!(store.read(address, &word(1), domain.borrowed()), keys(3, 7));
    assert_slot_eq!(
        store.read(address, &word(2), domain.borrowed()),
        TestValue::top()
    );

    // Replay with the newly observed baseline still joins the weak SSTORE.
    let mut observed = TestAccount::unknown();
    observed.storage.insert(U256::from(2), word(11));
    let mut refined = TestWorld::new(Fork::Osaka, "refined initial storage");
    refined.insert(address, observed).unwrap();
    let mut replayed = TestStore::new(
        &refined,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    replayed.write(address, &keys(1, 2), &word(7), domain.borrowed());
    assert_slot_eq!(
        replayed.read(address, &word(2), domain.borrowed()),
        keys(7, 11)
    );
    assert!(
        replayed
            .missing_initial_slots(&refined, address, &word(2))
            .is_empty()
    );
}

#[test]
fn unknown_alias_write_remains_weak_after_initial_rpc_refinement() {
    let (world, address) = partial_world();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.write(address, &word(1), &word(3), domain.borrowed());
    store.write(address, &TestValue::top(), &word(7), domain.borrowed());
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_slot_eq!(
        store.read(address, &word(4), domain.borrowed()),
        keys(7, 11)
    );
    assert_slot_eq!(store.read(address, &word(1), domain.borrowed()), keys(3, 7));
    assert_slot_eq!(
        store.read(address, &word(2), domain.borrowed()),
        TestValue::top()
    );
}

#[test]
fn havoc_replaces_initial_dependencies_for_exactly_its_scope() {
    let (world, address) = partial_world();
    let other = Address::repeat_byte(0x35);
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.havoc_account(address);
    assert!(
        store
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    assert_eq!(
        store.missing_initial_slots(&world, other, &word(1)),
        vec![U256::from(1)]
    );
    assert_slot_eq!(
        store.read(
            address,
            &word(4),
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        TestValue::top()
    );
    store.havoc_all();
    assert!(
        store
            .missing_initial_slots(&world, other, &word(1))
            .is_empty()
    );
}

#[test]
fn joins_keep_any_path_baseline_dependency_across_owner_and_global_defaults() {
    let (world, address) = partial_world();
    let other = Address::repeat_byte(0x35);
    let initial = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut strong = initial.clone();
    strong.write(address, &word(1), &TestValue::top(), domain.borrowed());
    let mut created = initial.clone();
    created.begin_creation(address);
    let mut havoc_account = initial.clone();
    havoc_account.havoc_account(address);
    let mut havoc_all = initial.clone();
    havoc_all.havoc_all();
    let variants = [initial, strong, created, havoc_account, havoc_all];
    for left in &variants {
        for right in &variants {
            let joined = left.join(right, domain.borrowed());
            let replay = left
                .summary_projection(domain.borrowed())
                .unwrap()
                .join(
                    &right.summary_projection(domain.borrowed()).unwrap(),
                    domain.borrowed(),
                    false,
                )
                .instantiate(domain.borrowed())
                .unwrap();
            for owner in [address, other] {
                let mut expected = left.missing_initial_slots(&world, owner, &keys(1, 2));
                expected.extend(right.missing_initial_slots(&world, owner, &keys(1, 2)));
                expected.sort_unstable();
                expected.dedup();
                assert_eq!(
                    joined.missing_initial_slots(&world, owner, &keys(1, 2)),
                    expected
                );
                assert_eq!(
                    replay.missing_initial_slots(&world, owner, &keys(1, 2)),
                    expected
                );
            }
        }
    }
}

#[test]
fn nested_rollback_restores_dependency_metadata_and_work() {
    let (world, address) = partial_world();
    let address3 = Address::from_word(U256::from(3).into());
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let initial = store.clone();
    let outer = store.snapshot();
    store.write(address, &word(1), &TestValue::top(), domain.borrowed());
    let after_strong = store.clone();
    let inner = store.snapshot();
    store.havoc_all();
    store.restore(inner);
    // Unknown effects may touch RIPEMD-160; its touch survives both reverts.
    // Every ordinary value, root and dependency still matches the snapshot.
    let mut expected = after_strong.clone();
    expected.may_touch_account(address3);
    assert_eq!(store, expected);
    assert_eq!(store.touched_in_transaction(address3), None);
    assert_eq!(store.work_size(), after_strong.work_size() + 1);
    assert!(
        store
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    store.restore(outer);
    let mut expected = initial.clone();
    expected.may_touch_account(address3);
    assert_eq!(store, expected);
    assert_eq!(store.touched_in_transaction(address3), None);
    assert_eq!(store.work_size(), initial.work_size() + 1);
    assert_eq!(
        store.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
}

#[test]
fn creation_storage_and_transient_storage_use_transaction_zero_baselines() {
    let (world, address) = partial_world();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    assert_slot_eq!(
        store.read_transient(address, &word(1), domain.borrowed()),
        word(0)
    );
    store.write_transient(address, &word(1), &word(9), domain.borrowed());
    assert_slot_eq!(
        store.read_transient(address, &word(1), domain.borrowed()),
        word(9)
    );
    assert_eq!(
        store.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
    store.begin_creation(address);
    store.write(address, &keys(1, 2), &word(7), domain.borrowed());
    assert!(
        store
            .missing_initial_slots(&world, address, &keys(1, 2))
            .is_empty()
    );
    assert_slot_eq!(store.read(address, &word(1), domain.borrowed()), keys(0, 7));
    assert_slot_eq!(
        store.read_transient(address, &word(1), domain.borrowed()),
        word(0)
    );
}

#[test]
fn guarded_join_and_domain_projection_preserve_the_joined_must_facts() {
    let (world, address) = partial_world();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let initial = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut strong = initial.clone();
    strong.write(address, &word(1), &TestValue::top(), domain.borrowed());
    let strong_root = strong.slot_root(address, false).unwrap();
    strong.project(domain.borrowed());
    assert_eq!(strong.slot_root(address, false).unwrap(), strong_root);
    assert!(
        strong
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    let mut joined = strong
        .join_guarded(
            &initial,
            domain.borrowed(),
            ExprId::fresh().unwrap(),
            domain.spec().relations().expressions(),
        )
        .unwrap();
    let joined_root = joined.slot_root(address, false).unwrap();
    joined.project(
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
            crate::domain::DomainSpec::new(
                NonZeroUsize::new(1).unwrap(),
                std::num::NonZeroUsize::new(8).unwrap(),
                std::num::NonZeroUsize::new(256).unwrap(),
            ),
        ))
        .borrowed(),
    );
    assert_eq!(joined.slot_root(address, false).unwrap(), joined_root);
    assert_eq!(
        joined.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
}
