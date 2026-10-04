//! Snapshot facts must survive input parsing without implicit zero assumptions.

use super::{InputError, parse};
use alloy_primitives::{Address, U256};
use evm_abstract::{
    domain::{Domain, Value},
    world::{Code, Store},
};

fn address(last: u64) -> Address {
    Address::from_word(U256::from(last).into())
}

#[test]
fn omitted_facts_stay_unknown_and_explicit_empty_code_is_known() {
    let world = parse(r#"{
        "fork":"osaka", "provenance":"offline:partial", "accounts":[
          {"address":"0x0000000000000000000000000000000000000101","code":"0x","storage":{"0x0":"0x7"}},
          {"address":"0x0000000000000000000000000000000000000200"}
        ]
    }"#).unwrap();
    assert!(matches!(
        world.account(address(0x101)).unwrap().code,
        Code::Empty
    ));
    assert!(matches!(
        world.account(address(0x200)).unwrap().code,
        Code::Unknown
    ));
    assert!(world.account(address(0x300)).is_none());
    let store = Store::new(&world);
    assert_eq!(
        store.read(
            address(0x101),
            &Value::constant(U256::ZERO),
            Domain::default()
        ),
        Value::constant(U256::from(7))
    );
    assert_eq!(
        store.read(
            address(0x101),
            &Value::constant(U256::from(1)),
            Domain::default()
        ),
        Value::top()
    );
    assert_eq!(store.read_balance(address(0x101)), Value::top());
}

#[test]
fn declared_complete_storage_makes_unlisted_slots_zero() {
    let world = parse(r#"{
        "fork":"osaka", "provenance":"offline:complete", "accounts":[
          {"address":"0x0000000000000000000000000000000000000101","code":"0x","storage_unknown":false,"balance":"0x9"}
        ]
    }"#).unwrap();
    let store = Store::new(&world);
    assert_eq!(
        store.read(
            address(0x101),
            &Value::constant(U256::from(1)),
            Domain::default()
        ),
        Value::constant(U256::ZERO)
    );
    assert_eq!(
        store.read_balance(address(0x101)),
        Value::constant(U256::from(9))
    );
}

#[test]
fn conflicting_account_or_normalized_slot_facts_are_rejected() {
    let duplicate_account = r#"{
        "fork":"osaka", "provenance":"offline:duplicate", "accounts":[
          {"address":"0x0000000000000000000000000000000000000101","code":"0x"},
          {"address":"0x0000000000000000000000000000000000000101","code":"0x00"}
        ]
    }"#;
    assert!(matches!(
        parse(duplicate_account),
        Err(InputError::Duplicate(_))
    ));
    let duplicate_slot = r#"{
        "fork":"osaka", "provenance":"offline:duplicate", "accounts":[
          {"address":"0x0000000000000000000000000000000000000101","code":"0x","storage":{"0x0":"0x1","0x00":"0x2"}}
        ]
    }"#;
    assert!(matches!(
        parse(duplicate_slot),
        Err(InputError::DuplicateSlot { .. })
    ));
}
