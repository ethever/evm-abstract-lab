//! Backend configuration is strict, offline and safe to expose through its catalogue.
use evm_abstract_protocol::{
    AnalysisInput, AnalyzeRequest, ApiErrorCode, BlockSelector, ErrorDetails, RpcInput,
};
use evm_abstract_server::rpc_providers::{ConfigError, Registry};

fn config(id: &str, name: &str, endpoint: &str) -> Vec<u8> {
    // Use JSON string encoding so controls and non-ASCII names exercise validation,
    // not accidental syntax errors in the fixture.
    format!(
        r#"{{"providers":[{{"id":{},"name":{},"endpoint":{}}}]}}"#,
        serde_json::to_string(id).unwrap(),
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(endpoint).unwrap(),
    )
    .into_bytes()
}

#[test]
fn catalogue_preserves_configured_order_and_hides_endpoints() {
    let providers = Registry::from_json(br#"{"providers":[{"id":"test","name":"Local test","endpoint":"http://alice:password@127.0.0.1:8545/private?api_key=secret"},{"id":"mainnet","name":"Ethereum mainnet","endpoint":"https://example.com"}]}"#).unwrap();
    let catalogue = providers.catalogue();
    assert_eq!(
        catalogue
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["test", "mainnet"]
    );
    let encoded = serde_json::to_string(&catalogue).unwrap();
    for secret in [
        "endpoint",
        "alice",
        "password",
        "private",
        "api_key",
        "secret",
        "127.0.0.1",
        "example.com",
    ] {
        assert!(!encoded.contains(secret), "catalogue disclosed {secret}");
    }
    assert!(Registry::default().catalogue().is_empty());
    assert!(
        Registry::from_json(br#"{"providers":[]}"#)
            .unwrap()
            .catalogue()
            .is_empty()
    );
}

#[test]
fn configuration_rejects_ambiguous_fields_and_invalid_provider_values() {
    let valid = String::from_utf8(config("mainnet", "Mainnet", "https://example.com")).unwrap();
    for bytes in [
        "{}".into(),
        r#"{"providers":null}"#.into(),
        r#"{"providers":[],"providers":[]}"#.into(),
        r#"{"providers":[],"secret-field":true}"#.into(),
        valid.replace(r#""id":"mainnet","#, ""),
        valid.replace(r#""name":"Mainnet""#, r#""name":"Mainnet","name":"Other""#),
        valid.replace("endpoint", "url"),
    ] {
        assert!(matches!(
            Registry::from_json(bytes.as_bytes()),
            Err(ConfigError::Json { .. })
        ));
    }
    let duplicate = br#"{"providers":[{"id":"same","name":"One","endpoint":"http://localhost"},{"id":"same","name":"Two","endpoint":"https://example.com"}]}"#;
    assert!(matches!(
        Registry::from_json(duplicate),
        Err(ConfigError::Provider { index: 1, .. })
    ));
    for id in [
        "",
        "with spaces",
        "http://example.com",
        "测试",
        &"x".repeat(65),
    ] {
        assert!(Registry::from_json(&config(id, "Mainnet", "https://example.com")).is_err());
    }
    for name in ["", "  ", "line\nbreak", &"x".repeat(129)] {
        assert!(Registry::from_json(&config("mainnet", name, "https://example.com")).is_err());
    }
    for endpoint in [
        "",
        "localhost:8545",
        "https:example.com",
        "https:example.com/path://segment",
        "http://",
        "http://[::1",
        "http://example.com:99999",
        "file:///tmp/key",
        "ftp://example.com",
        "https://example.com/#secret",
        "https://example.com/with space",
        "https://example.com/\nsecret",
    ] {
        assert!(
            Registry::from_json(&config("mainnet", "Mainnet", endpoint)).is_err(),
            "accepted {endpoint}"
        );
    }
    for endpoint in [
        "https://example.com/key?token=value",
        "http://127.0.0.1:8545",
        "http://[::1]:8545",
        "https://user:password@example.com",
    ] {
        assert!(Registry::from_json(&config("mainnet", "主网", endpoint)).is_ok());
    }
}

#[test]
fn config_diagnostics_never_echo_values_or_unknown_field_names() {
    for bytes in [
        config(
            "invalid/secret-id",
            "Mainnet",
            "https://user:secret-pass@example.com",
        ),
        config(
            "mainnet",
            "Mainnet",
            "ftp://user:secret-pass@example.com/secret-path",
        ),
        br#"{"providers":[],"secret-key": "secret-value"}"#.to_vec(),
        br#"{"providers":[{"id":"mainnet","name":"Mainnet","endpoint":secret-value}]}"#.to_vec(),
    ] {
        let error = match Registry::from_json(&bytes) {
            Ok(_) => panic!("expected invalid config"),
            Err(error) => error,
        };
        assert!(!format!("{error} {error:?}").contains("secret"));
    }
}

#[test]
fn standalone_analysis_requires_configured_provider_and_bytecode_stays_available() {
    let providers =
        Registry::from_json(&config("mainnet", "Mainnet", "https://example.com")).unwrap();
    providers
        .validate_request(&AnalyzeRequest::default())
        .unwrap();
    for id in ["", "unknown", "https://example.com", "MAINNET"] {
        let request = AnalyzeRequest {
            input: AnalysisInput::Rpc(RpcInput {
                provider_id: id.into(),
                address: "0x1111111111111111111111111111111111111111".into(),
                block: BlockSelector::Latest,
                accounts: Vec::new(),
            }),
            ..AnalyzeRequest::default()
        };
        let error =
            evm_abstract_server::analyze::analyze_with_providers(request, &providers).unwrap_err();
        assert_eq!(error.code, ApiErrorCode::InvalidRequest);
        let ErrorDetails::Validation(detail) = error.details else {
            panic!("typed validation")
        };
        assert_eq!(detail.field.as_deref(), Some("provider_id"));
        assert_eq!(detail.value, None);
    }
    assert!(evm_abstract_server::analyze::analyze(AnalyzeRequest::default()).is_ok());
}

#[test]
fn configured_transport_failure_keeps_endpoint_credentials_private() {
    // Keep the listener open without responding: a real local request must time
    // out, exercising URL removal in native transport failures and API mapping.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "http://secret-user:secret-pass@{}/secret-path?key=secret-token",
        listener.local_addr().unwrap()
    );
    let providers = Registry::from_json(&config("fixture", "Fixture", &endpoint)).unwrap();
    let mut request = AnalyzeRequest {
        input: AnalysisInput::Rpc(RpcInput {
            provider_id: "fixture".into(),
            address: "0x1111111111111111111111111111111111111111".into(),
            block: BlockSelector::Latest,
            accounts: Vec::new(),
        }),
        ..AnalyzeRequest::default()
    };
    request.limits.rpc_timeout_ms = 20;
    let error =
        evm_abstract_server::analyze::analyze_with_providers(request, &providers).unwrap_err();
    assert_eq!(error.code, ApiErrorCode::Rpc);
    let ErrorDetails::Rpc(detail) = &error.details else {
        panic!("typed RPC failure")
    };
    assert_eq!(
        detail.kind,
        evm_abstract_protocol::RpcFailureKind::Transport
    );
    let encoded = serde_json::to_string(&error).unwrap();
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains(&listener.local_addr().unwrap().to_string()));
}
