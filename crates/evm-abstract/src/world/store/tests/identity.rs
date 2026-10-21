//! Executable-state partitioning depends on effective facts, not sparse caches.

use super::super::*;
use {crate::Fork, crate::world::test_support::TestAccount};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn initial() -> TestStore {
    let mut world = TestWorld::new(Fork::Osaka, "code identity canonical defaults");
    world
        .insert(
            address(0x101),
            TestAccount::from_hex("00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    )
}

#[test]
fn explicit_defaults_and_touch_only_joins_preserve_code_identity() {
    let original = initial();
    let expected = original.code_identity();
    let mut equivalent = original.clone();
    for owner in [address(1), address(2), address(3)] {
        equivalent.codes.insert(owner, Code::Unknown);
        equivalent.existence.insert(owner, Existence::Unknown);
        equivalent.created.insert(owner, equivalent.created_default);
        equivalent
            .pending_destruction
            .insert(owner, equivalent.pending_destruction_default);
        equivalent.touch_account(owner);
        assert_eq!(equivalent.code_identity(), expected);
    }
    equivalent.may_touch_all();
    assert_eq!(equivalent.code_identity(), expected);
    for joined in [
        original.join(
            &equivalent,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        ),
        equivalent.join(
            &original,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        ),
    ] {
        assert_eq!(joined.code_identity(), expected);
    }
    equivalent.codes.clear();
    // Keep the real root code: removing it would change executable state.
    equivalent.codes = original.codes.clone();
    for owner in [address(1), address(2), address(3)] {
        equivalent.existence.remove(&owner);
        equivalent.created.remove(&owner);
        equivalent.pending_destruction.remove(&owner);
    }
    assert_eq!(equivalent.code_identity(), expected);
}

#[test]
fn unknown_global_lifecycle_defaults_remain_part_of_the_identity() {
    let mut identities = BTreeSet::new();
    for created in [Some(false), Some(true), None] {
        for pending in [Some(false), Some(true), None] {
            let mut store = initial();
            store.created_default = created;
            store.pending_destruction_default = pending;
            let before = store.code_identity();
            store.codes.insert(address(3), Code::Unknown);
            store.existence.insert(address(3), Existence::Unknown);
            store.created.insert(address(3), created);
            store.pending_destruction.insert(address(3), pending);
            assert_eq!(store.code_identity(), before);
            identities.insert(before);
        }
    }
    assert_eq!(identities.len(), 9);
}

#[test]
fn real_code_presence_and_owner_lifecycle_differences_still_partition() {
    let original = initial();
    let before = original.code_identity();
    for code in [
        Code::Empty,
        TestAccount::from_hex("00", Fork::Osaka).unwrap().code,
        Code::Delegation(address(0x300)),
    ] {
        let mut changed = original.clone();
        changed.codes.insert(address(3), code);
        assert_ne!(changed.code_identity(), before);
    }
    for existence in [Existence::Present, Existence::Absent] {
        let mut changed = original.clone();
        changed.existence.insert(address(3), existence);
        assert_ne!(changed.code_identity(), before);
    }
    for flag in [Some(true), None] {
        let mut created = original.clone();
        created.created.insert(address(3), flag);
        assert_ne!(created.code_identity(), before);
        let mut pending = original.clone();
        pending.pending_destruction.insert(address(3), flag);
        assert_ne!(pending.code_identity(), before);
    }
}
