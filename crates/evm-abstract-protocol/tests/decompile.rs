//! The independent request retains strict configuration and lossless identities.

use evm_abstract_protocol::{DecompileRequest, DecompileStrategy, decompile_model::EntityId};

#[test]
fn decompiler_request_roundtrips_without_execution_environment_fields() {
    let request = DecompileRequest::default();
    let json = serde_json::to_value(&request).unwrap();
    assert!(json.get("environment").is_none());
    assert_eq!(request.strategy, DecompileStrategy::Dispatcher);
    assert_eq!(
        serde_json::from_value::<DecompileRequest>(json).unwrap(),
        request
    );
}

#[test]
fn unknown_or_missing_configuration_fields_are_rejected() {
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["environment"] = serde_json::json!({});
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["limits"]["max_cloned_bloks"] = serde_json::json!(0);
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["limits"]
        .as_object_mut()
        .unwrap()
        .remove("max_work");
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
}

#[test]
fn entity_identity_remains_exact_above_javascript_safe_integer_range() {
    let identity = EntityId(u64::MAX);
    let json = serde_json::to_string(&identity).unwrap();
    assert_eq!(json, "\"18446744073709551615\"");
    assert_eq!(serde_json::from_str::<EntityId>(&json).unwrap(), identity);
    assert!(serde_json::from_str::<EntityId>("18446744073709551615").is_err());
}

#[test]
fn limits_are_named_objects_with_no_duplicate_fields() {
    let request = serde_json::to_value(DecompileRequest::default()).unwrap();
    let mut positional = request.clone();
    positional["limits"] = serde_json::json!([
        100000, 8192, 100000, 500000, 100000000, 100000, 8, 2048, 128, 2000000
    ]);
    assert!(serde_json::from_value::<DecompileRequest>(positional).is_err());
    let mut wrong_type = request.clone();
    wrong_type["limits"]["max_work"] = serde_json::json!("100");
    assert!(serde_json::from_value::<DecompileRequest>(wrong_type).is_err());
    let raw = serde_json::to_string(&request).unwrap();
    let duplicate = raw.replace("\"max_work\":", "\"max_work\":1,\"max_work\":");
    assert!(serde_json::from_str::<DecompileRequest>(&duplicate).is_err());
}
