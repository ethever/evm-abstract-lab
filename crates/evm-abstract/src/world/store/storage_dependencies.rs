//! RPC discovery requests initial facts only while a path still needs them.

use super::Store;
use crate::{
    Address, Fork, U256,
    domain::{Domain, Value},
    world::{Account, World},
};
use std::num::NonZeroUsize;

fn word(value: u64) -> Value {
    Value::constant(U256::from(value))
}

fn partial_world() -> (World, Address) {
    let address = Address::repeat_byte(0x31);
    let mut account = Account::unknown();
    account.storage.insert(U256::from(4), word(11));
    let mut world = World::new(Fork::Osaka, "initial storage dependencies");
    world.insert(address, account).unwrap();
    (world, address)
}

fn keys(left: u64, right: u64) -> Value {
    Domain::default().join(&word(left), &word(right))
}

#[test]
fn missing_requests_require_finite_keys_and_incomplete_initial_coverage() {
    let (mut world, address) = partial_world();
    let store = Store::new(&world);
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
            .missing_initial_slots(&world, address, &Value::top())
            .is_empty()
    );
    assert!(
        store
            .missing_initial_slots(&world, address, &Value::unknown_byte())
            .is_empty()
    );
    let single = Value::unsigned_range(U256::from(5), U256::from(5)).unwrap();
    assert_eq!(
        store.missing_initial_slots(&world, address, &single),
        vec![U256::from(5)]
    );
    let absent = Address::repeat_byte(0x32);
    let complete = Address::repeat_byte(0x33);
    let omitted = Address::repeat_byte(0x34);
    world.insert(absent, Account::absent()).unwrap();
    world.insert(complete, Account::empty()).unwrap();
    let store = Store::new(&world);
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
    let mut store = Store::new(&world);
    store.write(address, &word(1), &Value::top(), Domain::default());
    assert!(
        store
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_eq!(
        store.read(address, &word(1), Domain::default()),
        Value::top()
    );
}

#[test]
fn finite_weak_writes_preserve_baseline_but_do_not_restore_discarded_dependencies() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store.write(address, &word(1), &word(3), domain);
    store.write(address, &keys(1, 2), &word(7), domain);
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_eq!(store.read(address, &word(1), domain), keys(3, 7));
    assert_eq!(store.read(address, &word(2), domain), Value::top());

    // Replay with the newly observed baseline still joins the weak SSTORE.
    let mut observed = Account::unknown();
    observed.storage.insert(U256::from(2), word(11));
    let mut refined = World::new(Fork::Osaka, "refined initial storage");
    refined.insert(address, observed).unwrap();
    let mut replayed = Store::new(&refined);
    replayed.write(address, &keys(1, 2), &word(7), domain);
    assert_eq!(replayed.read(address, &word(2), domain), keys(7, 11));
    assert!(
        replayed
            .missing_initial_slots(&refined, address, &word(2))
            .is_empty()
    );
}

#[test]
fn unknown_alias_write_remains_weak_after_initial_rpc_refinement() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store.write(address, &word(1), &word(3), domain);
    store.write(address, &Value::top(), &word(7), domain);
    assert_eq!(
        store.missing_initial_slots(&world, address, &keys(1, 2)),
        vec![U256::from(2)]
    );
    assert_eq!(store.read(address, &word(4), domain), keys(7, 11));
    assert_eq!(store.read(address, &word(1), domain), keys(3, 7));
    assert_eq!(store.read(address, &word(2), domain), Value::top());
}

#[test]
fn havoc_replaces_initial_dependencies_for_exactly_its_scope() {
    let (world, address) = partial_world();
    let other = Address::repeat_byte(0x35);
    let mut store = Store::new(&world);
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
    assert_eq!(
        store.read(address, &word(4), Domain::default()),
        Value::top()
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
    let initial = Store::new(&world);
    let domain = Domain::default();
    let mut strong = initial.clone();
    strong.write(address, &word(1), &Value::top(), domain);
    let mut created = initial.clone();
    created.begin_creation(address);
    let mut havoc_account = initial.clone();
    havoc_account.havoc_account(address);
    let mut havoc_all = initial.clone();
    havoc_all.havoc_all();
    let variants = [initial, strong, created, havoc_account, havoc_all];
    for left in &variants {
        for right in &variants {
            let joined = left.join(right, domain);
            for owner in [address, other] {
                let mut expected = left.missing_initial_slots(&world, owner, &keys(1, 2));
                expected.extend(right.missing_initial_slots(&world, owner, &keys(1, 2)));
                expected.sort_unstable();
                expected.dedup();
                assert_eq!(
                    joined.missing_initial_slots(&world, owner, &keys(1, 2)),
                    expected
                );
            }
        }
    }
}

#[test]
fn nested_rollback_restores_dependency_metadata_and_work() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    let initial = store.clone();
    let outer = store.snapshot();
    store.write(address, &word(1), &Value::top(), domain);
    let after_strong = store.clone();
    let inner = store.snapshot();
    store.havoc_all();
    store.restore(inner);
    assert_eq!(store, after_strong);
    assert_eq!(store.work_size(), after_strong.work_size());
    assert!(
        store
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    store.restore(outer);
    assert_eq!(store, initial);
    assert_eq!(store.work_size(), initial.work_size());
    assert_eq!(
        store.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
}

#[test]
fn creation_storage_and_transient_storage_use_transaction_zero_baselines() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let mut store = Store::new(&world);
    assert_eq!(store.read_transient(address, &word(1), domain), word(0));
    store.write_transient(address, &word(1), &word(9), domain);
    assert_eq!(store.read_transient(address, &word(1), domain), word(9));
    assert_eq!(
        store.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
    store.begin_creation(address);
    store.write(address, &keys(1, 2), &word(7), domain);
    assert!(
        store
            .missing_initial_slots(&world, address, &keys(1, 2))
            .is_empty()
    );
    assert_eq!(store.read(address, &word(1), domain), keys(0, 7));
    assert_eq!(store.read_transient(address, &word(1), domain), word(0));
}

#[test]
fn widening_and_domain_projection_preserve_the_joined_must_facts() {
    let (world, address) = partial_world();
    let domain = Domain::default();
    let initial = Store::new(&world);
    let mut strong = initial.clone();
    strong.write(address, &word(1), &Value::top(), domain);
    strong.widen(&initial, domain);
    assert!(
        strong
            .missing_initial_slots(&world, address, &word(1))
            .is_empty()
    );
    let mut joined = strong.join(&initial, domain);
    joined.widen(&strong, domain);
    joined.project(Domain::new(NonZeroUsize::new(1).unwrap()));
    assert_eq!(
        joined.missing_initial_slots(&world, address, &word(1)),
        vec![U256::from(1)]
    );
}
