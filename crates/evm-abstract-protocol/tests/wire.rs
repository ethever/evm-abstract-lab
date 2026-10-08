//! Check the shared request contract independently of the native engine.

use evm_abstract_protocol::{AnalyzeReply, AnalyzeRequest, ApiError, ApiErrorCode};

#[test]
fn request_and_failure_roundtrip_without_erased_payloads() {
    let request = AnalyzeRequest::default();
    assert_eq!(
        serde_json::from_slice::<AnalyzeRequest>(&serde_json::to_vec(&request).unwrap()).unwrap(),
        request
    );
    let reply = AnalyzeReply {
        result: Err(ApiError {
            code: ApiErrorCode::InvalidBytecode,
            message: "invalid hex digit".into(),
        }),
    };
    assert_eq!(
        serde_json::from_slice::<AnalyzeReply>(&serde_json::to_vec(&reply).unwrap()).unwrap(),
        reply
    );
}

#[test]
fn misspelled_duplicate_missing_and_unknown_fields_are_rejected() {
    for json in [
        r#"{"bytecode":"00","fork":"future","limits":{"max_states":1,"max_transfers":1,"context_depth":0,"max_constants":8}}"#,
        r#"{"bytecode":"00","bytecode":"01","fork":"Osaka","limits":{"max_states":1,"max_transfers":1,"context_depth":0,"max_constants":8}}"#,
        r#"{"bytecode":"00","fork":"Osaka"}"#,
        r#"{"bytecode":"00","fork":"Osaka","limits":{"max_states":1,"max_transfers":1,"context_depth":0,"max_constant":8}}"#,
    ] {
        assert!(
            serde_json::from_str::<AnalyzeRequest>(json).is_err(),
            "{json}"
        );
    }
}

#[test]
fn optional_fields_are_required_and_duplicate_null_is_rejected() {
    let valid = r#"{"from":null,"pc":null,"kind":"Memory","detail":"bounded"}"#;
    let decoded: evm_abstract_protocol::Frontier = serde_json::from_str(valid).unwrap();
    assert_eq!(decoded.from, None);
    assert_eq!(decoded.pc, None);
    for json in [
        r#"{"from":null,"from":null,"pc":null,"kind":"Memory","detail":"bounded"}"#,
        r#"{"from":null,"kind":"Memory","detail":"bounded"}"#,
    ] {
        assert!(serde_json::from_str::<evm_abstract_protocol::Frontier>(json).is_err());
    }
}
