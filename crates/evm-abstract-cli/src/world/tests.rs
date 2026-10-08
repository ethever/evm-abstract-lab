//! Snapshot facts must survive input parsing without implicit zero assumptions.

use super::{InputError, parse};
use alloy_primitives::{Address, U256};
use evm_abstract::{
    domain::{AbstractValue, Domain},
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
            &AbstractValue::constant(U256::ZERO),
            Domain::default()
        ),
        AbstractValue::constant(U256::from(7))
    );
    assert_eq!(
        store.read(
            address(0x101),
            &AbstractValue::constant(U256::from(1)),
            Domain::default()
        ),
        AbstractValue::top()
    );
    assert_eq!(store.read_balance(address(0x101)), AbstractValue::top());
    assert_eq!(
        world.account(address(0x101)).unwrap().nonce,
        AbstractValue::top()
    );
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
            &AbstractValue::constant(U256::from(1)),
            Domain::default()
        ),
        AbstractValue::constant(U256::ZERO)
    );
    assert_eq!(
        store.read_balance(address(0x101)),
        AbstractValue::constant(U256::from(9))
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
        AbstractValue::constant(U256::from(1))
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

fn json_error(input: &str) -> String {
    match parse(input) {
        Err(InputError::Json(error)) => error.to_string(),
        Err(error) => panic!("expected JSON error, observed {error:?}"),
        Ok(_) => panic!("invalid JSON input was accepted: {input}"),
    }
}

#[test]
fn snapshot_objects_reject_unknown_fields_at_each_input_level() {
    let inputs = [
        r#"{"fork":"osaka","provenance":"test","accounts":[],"extra":0}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","extra":0}]}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline","label":"test","extra":0}}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline","label":"test","chain_id":"0x1"}}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"chain","label":"test"}}"#,
    ];
    for input in inputs {
        assert!(json_error(input).contains("unknown field"), "{input}");
    }
}

#[test]
fn required_snapshot_and_identity_fields_remain_required() {
    let cases = [
        (r#"{"provenance":"test","accounts":[]}"#, "fork"),
        (r#"{"fork":"osaka","accounts":[]}"#, "provenance"),
        (r#"{"fork":"osaka","provenance":"test"}"#, "accounts"),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[{}]}"#,
            "address",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"label":"test"}}"#,
            "kind",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline"}}"#,
            "label",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"chain"}}"#,
            "chain_id",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"chain","chain_id":"0x1"}}"#,
            "block_hash",
        ),
    ];
    for (input, field) in cases {
        assert!(
            json_error(input).contains(&format!("missing field `{field}`")),
            "{input}"
        );
    }
}

#[test]
fn null_optional_fields_match_omitted_unknown_facts() {
    let omitted = parse(
        r#"{
        "fork":"osaka","provenance":"test",
        "accounts":[{"address":"0x0000000000000000000000000000000000000101"}]
    }"#,
    )
    .unwrap();
    let explicit_null = parse(
        r#"{
        "fork":"osaka","provenance":"test","identity":null,"fingerprint":null,
        "accounts":[{"address":"0x0000000000000000000000000000000000000101",
            "code":null,"balance":null,"nonce":null,"existence":null,"code_hash":null}]
    }"#,
    )
    .unwrap();
    assert_eq!(omitted.fingerprint(), explicit_null.fingerprint());
    let account = explicit_null.account(address(0x101)).unwrap();
    assert!(account.storage.is_empty());
    assert!(account.storage_unknown);
    assert!(matches!(account.code, Code::Unknown));
}

#[test]
fn explicit_null_does_not_default_required_fields_or_storage() {
    let inputs = [
        r#"{"fork":null,"provenance":"test","accounts":[]}"#,
        r#"{"fork":"osaka","provenance":null,"accounts":[]}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":null}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[{"address":null}]}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","storage":null}]}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","storage_unknown":null}]}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":null,"label":"test"}}"#,
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline","label":null}}"#,
    ];
    for input in inputs {
        assert!(json_error(input).contains("invalid type: null"), "{input}");
    }
}

#[test]
fn duplicate_fields_are_rejected_even_when_the_first_value_is_null() {
    for field in ["identity", "fingerprint"] {
        let input = format!(
            r#"{{"fork":"osaka","provenance":"test","accounts":[],"{field}":null,"{field}":null}}"#
        );
        assert!(json_error(&input).contains(&format!("duplicate field `{field}`")));
    }
    for field in ["code", "balance", "nonce", "existence", "code_hash"] {
        let input = format!(
            r#"{{"fork":"osaka","provenance":"test","accounts":[{{"address":"0x0000000000000000000000000000000000000101","{field}":null,"{field}":null}}]}}"#
        );
        assert!(json_error(&input).contains(&format!("duplicate field `{field}`")));
    }
    let cases = [
        (
            r#"{"fork":"osaka","fork":"osaka","provenance":"test","accounts":[]}"#,
            "fork",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","provenance":"test","accounts":[]}"#,
            "provenance",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"accounts":[]}"#,
            "accounts",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","address":"0x0000000000000000000000000000000000000101"}]}"#,
            "address",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","storage":{},"storage":{}}]}"#,
            "storage",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[{"address":"0x0000000000000000000000000000000000000101","storage_unknown":true,"storage_unknown":false}]}"#,
            "storage_unknown",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline","label":"test","kind":"offline"}}"#,
            "kind",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"label":"test","label":"test","kind":"offline"}}"#,
            "label",
        ),
        (
            r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"chain","chain_id":"0x1","chain_id":"0x1"}}"#,
            "chain_id",
        ),
    ];
    for (input, field) in cases {
        assert!(
            json_error(input).contains(&format!("duplicate field `{field}`")),
            "{input}"
        );
    }
}

#[test]
fn identity_tag_can_follow_variant_fields_and_rejects_unknown_kinds() {
    let before = parse(r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"offline","label":"example"}}"#).unwrap();
    let after = parse(r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"label":"example","kind":"offline"}}"#).unwrap();
    assert_eq!(before.fingerprint(), after.fingerprint());

    let block_hash = alloy_primitives::B256::repeat_byte(0x11);
    let chain = format!(
        r#"{{"fork":"osaka","provenance":"test","accounts":[],"identity":{{"block_hash":"{block_hash}","kind":"chain","chain_id":"0x1"}}}}"#
    );
    assert_eq!(
        parse(&chain).unwrap().identity(),
        &SnapshotIdentity::Chain {
            chain_id: U256::from(1),
            block_hash,
        }
    );
    let unknown =
        r#"{"fork":"osaka","provenance":"test","accounts":[],"identity":{"kind":"latest"}}"#;
    assert!(json_error(unknown).contains("unknown variant `latest`"));
}

#[test]
fn sequence_inputs_preserve_the_existing_snapshot_format() {
    let map = parse(
        r#"{
        "fork":"osaka","provenance":"test","identity":{"kind":"offline","label":"example"},
        "accounts":[{"address":"0x0000000000000000000000000000000000000101"}]
    }"#,
    )
    .unwrap();
    let sequence = parse(
        r#"[
        "osaka","test",["offline","example"],null,
        [["0x0000000000000000000000000000000000000101",null,{},true,null,null,null,null]]
    ]"#,
    )
    .unwrap();
    assert_eq!(map.fingerprint(), sequence.fingerprint());

    assert!(json_error(r#"["osaka","test",null,null]"#).contains("invalid length 4"));
    assert!(json_error(r#"["osaka","test",[],null,[]]"#).contains("missing field `kind`"));
    assert!(json_error(r#"["osaka","test",["offline"],null,[]]"#).contains("invalid length 0"));
    let short_account =
        r#"["osaka","test",null,null,[["0x0000000000000000000000000000000000000101",null]]]"#;
    assert!(json_error(short_account).contains("invalid length 4"));
    let extra_identity = r#"["osaka","test",["offline","example","extra"],null,[]]"#;
    assert!(!json_error(extra_identity).is_empty());
}
