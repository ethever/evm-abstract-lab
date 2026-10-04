//! Snapshot consistency remains invariant before execution is constructed.

use alloy_primitives::{B256, keccak256};
use evm_abstract::{
    Address, Fork, U256,
    domain::Value,
    world::{Account, World, WorldError},
};

#[test]
fn fingerprint_is_order_independent_and_binds_snapshot_and_nonce() {
    let left_address = Address::repeat_byte(1);
    let right_address = Address::repeat_byte(2);
    let account = Account::from_hex("00", Fork::Osaka).unwrap();
    let mut left = World::anchored(Fork::Osaka, U256::from(1), B256::repeat_byte(3), "export-a");
    left.insert(left_address, account.clone()).unwrap();
    left.insert(right_address, Account::empty()).unwrap();
    let mut right = World::anchored(Fork::Osaka, U256::from(1), B256::repeat_byte(3), "export-b");
    right.insert(right_address, Account::empty()).unwrap();
    right.insert(left_address, account.clone()).unwrap();
    assert_eq!(left.fingerprint(), right.fingerprint());
    let mut changed = World::anchored(Fork::Osaka, U256::from(2), B256::repeat_byte(3), "export-a");
    changed.insert(left_address, account.clone()).unwrap();
    changed.insert(right_address, Account::empty()).unwrap();
    assert_ne!(left.fingerprint(), changed.fingerprint());
    let mut changed = World::anchored(Fork::Osaka, U256::from(1), B256::repeat_byte(4), "export-a");
    changed.insert(left_address, account.clone()).unwrap();
    changed.insert(right_address, Account::empty()).unwrap();
    assert_ne!(left.fingerprint(), changed.fingerprint());
    let mut changed = World::anchored(Fork::Osaka, U256::from(1), B256::repeat_byte(3), "export-a");
    let mut account = account;
    account.nonce = Value::constant(U256::from(7));
    changed.insert(left_address, account).unwrap();
    changed.insert(right_address, Account::empty()).unwrap();
    assert_ne!(left.fingerprint(), changed.fingerprint());
}

#[test]
fn conflicting_or_mismatched_facts_are_rejected_without_mutation() {
    let address = Address::repeat_byte(1);
    let mut world = World::new(Fork::Osaka, "synthetic");
    let account = Account::from_hex("00", Fork::Osaka).unwrap();
    world
        .insert_with_code_hash(address, account.clone(), keccak256([0]))
        .unwrap();
    let original = world.fingerprint();
    assert!(world.insert(address, account).unwrap().is_some());
    assert!(matches!(
        world.insert(address, Account::empty()),
        Err(WorldError::Conflict { .. })
    ));
    assert!(matches!(
        world.insert_with_code_hash(Address::repeat_byte(2), Account::empty(), B256::ZERO),
        Err(WorldError::CodeHash { .. })
    ));
    assert_eq!(world.fingerprint(), original);
}

#[test]
fn delegated_marker_hash_binds_original_bytes() {
    let address = Address::repeat_byte(1);
    let target = Address::repeat_byte(2);
    let bytes = format!("ef0100{}", alloy_primitives::hex::encode(target));
    let account = Account::from_hex(&bytes, Fork::Osaka).unwrap();
    let expected = keccak256(alloy_primitives::hex::decode(&bytes).unwrap());
    let mut world = World::new(Fork::Osaka, "delegation");
    world
        .insert_with_code_hash(address, account, expected)
        .unwrap();
    assert_eq!(world.code_hash(address), Some(expected));
}
