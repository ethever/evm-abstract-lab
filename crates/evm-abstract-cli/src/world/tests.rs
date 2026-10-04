//! Snapshot facts must survive input parsing without implicit zero assumptions.

use super::{InputError, parse};
use alloy_primitives::{Address, U256};
use evm_abstract::{
    domain::{Domain, Value},
    world::{Code, Existence, SnapshotIdentity, Store},
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
    assert_eq!(world.account(address(0x101)).unwrap().nonce, Value::top());
    assert_eq!(
        world.account(address(0x101)).unwrap().existence,
        Existence::Unknown
    );
    assert!(matches!(world.identity(), SnapshotIdentity::Offline { .. }));
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
fn anchored_snapshot_validates_block_identity_code_hash_and_fingerprint() {
    let code_hash = alloy_primitives::keccak256([0]);
    let block_hash = alloy_primitives::B256::repeat_byte(0x11);
    let input = format!(
        r#"{{
        "fork":"osaka", "provenance":"trusted-export",
        "identity":{{"kind":"chain","chain_id":"0x1","block_hash":"{block_hash}"}},
        "accounts":[{{"address":"0x0000000000000000000000000000000000000101", "code":"0x00", "code_hash":"{code_hash}", "nonce":"0x1", "existence":"present"}}]
    }}"#
    );
    let world = parse(&input).unwrap();
    assert_eq!(
        world.identity(),
        &SnapshotIdentity::Chain {
            chain_id: U256::from(1),
            block_hash
        }
    );
    assert_eq!(
        world.account(address(0x101)).unwrap().nonce,
        Value::constant(U256::from(1))
    );
    let mut json: serde_json::Value = serde_json::from_str(&input).unwrap();
    json["fingerprint"] = serde_json::json!(world.fingerprint());
    assert_eq!(
        parse(&json.to_string()).unwrap().fingerprint(),
        world.fingerprint()
    );
    json["accounts"][0]["nonce"] = serde_json::json!("0x2");
    assert!(matches!(
        parse(&json.to_string()),
        Err(InputError::Fingerprint { .. })
    ));
}

#[test]
fn anchored_missing_or_mismatched_code_hash_and_invalid_metadata_are_rejected() {
    let base = serde_json::json!({
        "fork":"osaka", "provenance":"trusted-export",
        "identity":{"kind":"chain","chain_id":"0x1","block_hash":alloy_primitives::B256::repeat_byte(0x11)},
        "accounts":[{"address":"0x0000000000000000000000000000000000000101", "code":"0x00"}],
    });
    assert!(matches!(
        parse(&base.to_string()),
        Err(InputError::MissingCodeHash(_))
    ));
    let mut json = base.clone();
    json["accounts"][0]["code_hash"] = serde_json::json!(alloy_primitives::keccak256([]));
    assert!(matches!(
        parse(&json.to_string()),
        Err(InputError::World(
            evm_abstract::world::WorldError::CodeHash { .. }
        ))
    ));
    json["accounts"][0].as_object_mut().unwrap().remove("code");
    assert!(matches!(
        parse(&json.to_string()),
        Err(InputError::World(
            evm_abstract::world::WorldError::UnknownCodeHash(_)
        ))
    ));
    let mut json = base.clone();
    json["identity"]["block_hash"] = serde_json::json!("latest");
    assert!(matches!(
        parse(&json.to_string()),
        Err(InputError::Hash { .. })
    ));
    let mut json = base;
    json["identity"]["chain_id"] = serde_json::json!("1");
    assert!(matches!(
        parse(&json.to_string()),
        Err(InputError::WordSyntax { .. })
    ));
}

#[test]
fn offline_explicit_identity_and_duplicate_json_storage_keys_are_checked() {
    let input = r#"{
        "fork":"osaka", "provenance":"author-description",
        "identity":{"kind":"offline","label":"synthetic-demo"},
        "accounts":[{"address":"0x0000000000000000000000000000000000000101", "code":"0x00"}]
    }"#;
    assert_eq!(
        parse(input).unwrap().identity(),
        &SnapshotIdentity::Offline {
            label: "synthetic-demo".to_owned()
        }
    );
    let duplicated = r#"{
        "fork":"osaka", "provenance":"bad-input",
        "accounts":[{"address":"0x0000000000000000000000000000000000000101", "storage":{"0x1":"0x7","0x1":"0x8"}}]
    }"#;
    assert!(matches!(parse(duplicated), Err(InputError::Json(_))));
}

#[test]
fn absent_account_requires_all_zero_observations() {
    let mut input = serde_json::json!({
        "fork":"osaka", "provenance":"offline:absence",
        "accounts":[{"address":"0x0000000000000000000000000000000000000101", "code":"0x", "existence":"absent", "balance":"0x0", "nonce":"0x0", "storage_unknown":false}],
    });
    assert_eq!(
        parse(&input.to_string())
            .unwrap()
            .account(address(0x101))
            .unwrap()
            .existence,
        Existence::Absent
    );
    input["accounts"][0]["nonce"] = serde_json::json!("0x1");
    assert!(matches!(
        parse(&input.to_string()),
        Err(InputError::World(
            evm_abstract::world::WorldError::InvalidAbsence(_)
        ))
    ));
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
