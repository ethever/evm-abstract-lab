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

// Literal pre-cache encoding for the fixture's one owner. Runtime records
// deliberately hash the supplied bytes directly, independently of Program.
fn runtime_record(bytes: &[u8]) -> Vec<u8> {
    let mut record = vec![1];
    record.extend(keccak256(bytes).as_slice());
    record
}

fn literal_identity(code_record: &[u8], lifecycle: [u8; 3]) -> B256 {
    let mut record = vec![1, 1]; // Both global lifecycle defaults are false.
    record.extend(address(0x101).as_slice());
    record.extend(code_record);
    record.extend(lifecycle);
    keccak256(record)
}

#[test]
fn every_code_variant_keeps_the_literal_identity_encoding() {
    let mut delegation = vec![2];
    delegation.extend(address(0x300).as_slice());
    let mut store = initial();
    let mut identities = BTreeSet::new();
    for (code, record) in [
        (Code::Unknown, vec![4]),
        (Code::Empty, vec![3]),
        (
            Code::Runtime(Program::decode(&[]).unwrap()),
            runtime_record(&[]),
        ),
        (
            Code::Runtime(Program::decode(&[0]).unwrap()),
            runtime_record(&[0]),
        ),
        (Code::Delegation(address(0x300)), delegation),
    ] {
        store.codes.insert(address(0x101), code);
        let expected = literal_identity(&record, [1, 1, 1]);
        assert_eq!(store.code_identity(), expected);
        assert_eq!(store.clone().code_identity(), expected);
        identities.insert(expected);
    }
    assert_eq!(identities.len(), 5);
}

#[test]
fn runtime_digest_preserves_deployment_deletion_and_rollback_identities() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let owner = address(0x101);
    let mut store = initial();
    let original = store.snapshot();
    let original_identity = literal_identity(&runtime_record(&[0]), [1, 1, 1]);
    assert_eq!(store.code_identity(), original_identity);

    store.begin_creation(owner);
    assert_eq!(store.code_identity(), literal_identity(&[3], [1, 2, 1]));
    let runtime = Program::from_hex("61ab").unwrap();
    store.deploy_code(owner, runtime.clone());
    let deployed_identity = literal_identity(&runtime_record(&[0x61, 0xab]), [1, 2, 1]);
    // Quote before the first identity call initializes this program's digest.
    let quoted_bytes = store.code_identity_bytes();
    assert_eq!(quoted_bytes, runtime.byte_len());
    assert_eq!(store.code_identity(), deployed_identity);
    let deployed = store.snapshot();
    runtime.bytes_digest();
    assert_eq!(store.code_identity_bytes(), quoted_bytes);
    assert_eq!(store.code_identity(), deployed_identity);

    // PUSH padding gives these programs the same listing but different source
    // bytes. Replacing runtime at the same owner must change the key.
    let replaced = Program::from_hex("61ab00").unwrap();
    assert_eq!(runtime.blocks(), replaced.blocks());
    store.deploy_code(owner, replaced);
    assert_eq!(
        store.code_identity(),
        literal_identity(&runtime_record(&[0x61, 0xab, 0]), [1, 2, 1])
    );
    assert_ne!(store.code_identity(), deployed_identity);
    assert_eq!(deployed.state().code_identity(), deployed_identity);

    store.restore(deployed);
    store.selfdestruct(owner, owner, domain.borrowed());
    assert_eq!(
        store.code_identity(),
        literal_identity(&runtime_record(&[0x61, 0xab]), [1, 2, 2])
    );
    store.finalize_transaction(domain.borrowed()).unwrap();
    assert_eq!(store.code_identity(), literal_identity(&[3], [2, 1, 1]));
    store.restore(original);
    assert_eq!(store.code_identity(), original_identity);
    store.deploy_code(owner, Program::decode(&[]).unwrap());
    assert_eq!(store.code_identity(), literal_identity(&[3], [1, 1, 1]));
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
