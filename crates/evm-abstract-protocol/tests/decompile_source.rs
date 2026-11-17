//! Canonical byte/configuration identity is shared by browser and server.
use evm_abstract_protocol::{
    DecompileHashSource, DecompileOpenRequest, DecompileRequest, DecompileSettings,
    DecompileSourceInput, Fork, decompile_sha256,
};

#[test]
fn source_digest_is_sha256_of_bytes_and_policy_digest_covers_the_complete_settings() {
    assert_eq!(
        decompile_sha256(&[]),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        decompile_sha256(&[0]),
        "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
    );
    assert_ne!(decompile_sha256(&[0]), decompile_sha256(b"00"));
    let settings = DecompileSettings::default();
    let key = settings.sha256();
    let mut fork = settings.clone();
    fork.fork = Fork::Cancun;
    assert_ne!(key, fork.sha256());
    let mut resource = settings.clone();
    resource.config.limits.max_work = 123;
    assert_ne!(key, resource.sha256());
    let mut reading = settings.clone();
    reading.config.readability.profile =
        evm_abstract_protocol::decompile_model::ReadabilityProfile::Compact;
    assert_ne!(key, reading.sha256());
    let mut scope = settings.clone();
    scope.config.execution_domain =
        evm_abstract_protocol::decompile_model::ExecutionDomain::EthereumTransactions;
    assert_ne!(key, scope.sha256());
    let request = settings.clone().into_request("0x00".into());
    assert_eq!(DecompileSettings::from_request(&request), settings);
    assert_eq!(
        DecompileSettings::from_request(&DecompileRequest {
            bytecode: "deadbeef".into(),
            ..request
        })
        .sha256(),
        key
    );
}

#[test]
fn source_open_has_strict_lowercase_tags_and_omitted_defaults_are_explicit() {
    let source = DecompileSourceInput::Sha256(DecompileHashSource {
        sha256: "a".repeat(64),
        config_sha256: None,
    });
    let request = DecompileOpenRequest {
        source,
        settings: None,
    };
    let encoded = serde_json::to_value(&request).unwrap();
    assert!(encoded["source"].get("sha256").is_some());
    assert_eq!(
        serde_json::from_value::<DecompileOpenRequest>(encoded).unwrap(),
        request
    );
    let simple = format!(
        "{{\"source\":{{\"sha256\":{{\"sha256\":\"{}\"}}}}}}",
        "a".repeat(64)
    );
    assert_eq!(
        serde_json::from_str::<DecompileOpenRequest>(&simple).unwrap(),
        request
    );
    assert!(
        serde_json::from_str::<DecompileOpenRequest>(
            "{\"source\":{\"bytecode\":\"00\"},\"source\":{\"bytecode\":\"01\"}}"
        )
        .is_err()
    );
}
