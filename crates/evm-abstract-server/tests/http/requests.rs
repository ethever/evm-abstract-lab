//! Exhaustive malformed-limit variants belong at the real TCP/API boundary.
//! Chromium retains a representative rejection plus its form/error recovery.
use super::*;

#[test]
fn malformed_limit_matrix_is_rejected_before_admission_and_valid_input_recovers() {
    let assets = Assets::new();
    let pool = pool();
    let original = serde_json::to_value(AnalyzeRequest::default()).unwrap();
    for (name, field, value) in [
        ("numeric-string", "max_transfers", Some("\"1\"")),
        (
            "numeric-overflow",
            "max_transfers",
            Some("18446744073709551616"),
        ),
        ("boolean-string", "root_terminal_coverage", Some("\"true\"")),
        ("boolean-number", "root_terminal_coverage", Some("1")),
        ("unknown-limit", "root_terminal_coverag", Some("true")),
        ("obsolete-selector", "domain_profile", Some("\"Product\"")),
        ("obsolete-model", "numeric_model", Some("\"ConstantsOnly\"")),
        ("missing-model", "numeric_model", None),
    ] {
        let mut invalid = original.clone();
        let limits = invalid["limits"].as_object_mut().unwrap();
        // Preserve the out-of-u64 token through serialization; Value cannot
        // represent that integer losslessly without arbitrary_precision.
        limits.remove(field);
        let mut body = serde_json::to_string(&invalid).unwrap();
        if let Some(value) = value {
            let start = body.find("\"limits\":{").unwrap() + "\"limits\":{".len();
            body.insert_str(start, &format!("\"{field}\":{value},"));
        }
        let request = format!(
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (headers, reply) =
            decode_job(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        assert!(headers.starts_with("HTTP/1.1 400"), "{name}: {headers}");
        let error = reply.result.unwrap_err();
        assert_eq!(error.code, ApiErrorCode::InvalidRequest, "{name}");
        let ErrorDetails::Validation(detail) = error.details else {
            panic!("{name}: missing typed validation")
        };
        assert_eq!(
            detail.kind,
            evm_abstract_protocol::ValidationErrorKind::Json,
            "{name}"
        );
    }
    // Rejections did not poison admission or the transport. Use the same pool
    // for a real accepted job, then require its structured response to parse.
    let body = serde_json::to_string(&AnalyzeRequest::default()).unwrap();
    let request = format!(
        "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (headers, reply) = decode_job(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
    assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
    assert!(reply.result.is_ok());
    pool.shutdown().unwrap();
}
