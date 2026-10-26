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

#[test]
fn detail_region_omission_and_explicit_zero_one_cross_the_actual_http_boundary() {
    let assets = Assets::new();
    let pool = pool();
    for bound in [None, Some(1_u64), Some(0)] {
        let mut body = serde_json::to_value(AnalyzeRequest::default()).unwrap();
        if let Some(bound) = bound {
            body["limits"]["max_detail_regions"] = bound.into();
        } else {
            body["limits"]
                .as_object_mut()
                .unwrap()
                .remove("max_detail_regions");
        }
        let body = serde_json::to_string(&body).unwrap();
        let bytes = format!(
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (headers, reply) = decode_job(&exchange_with_pool(bytes.as_bytes(), &assets.0, &pool));
        assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
        let id = reply.result.unwrap().id;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (_, reply) = decode_job(&exchange_with_pool(
                format!("GET /api/tasks/{id} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
                &assets.0,
                &pool,
            ));
            let status = reply.result.unwrap();
            if status.state == JobState::Completed {
                break;
            }
            assert!(!matches!(status.state, JobState::Failed(_)), "{status:?}");
            assert!(Instant::now() < deadline, "{status:?}");
            thread::sleep(Duration::from_millis(1));
        }
        let (_, reply) = decode(&exchange_with_pool(
            format!("GET /api/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            &assets.0,
            &pool,
        ));
        let report = reply.result.unwrap().report;
        report.validate().unwrap();
        let coverage = &report.hybrid.as_ref().unwrap().coverage;
        let expected = bound.unwrap_or(1);
        assert!(coverage.baseline_current);
        assert_eq!(coverage.requested_detail_regions, expected);
        assert_eq!(
            report
                .global
                .as_ref()
                .unwrap()
                .metadata
                .limits
                .max_detail_regions,
            expected
        );
        assert_eq!(
            report.global.as_ref().unwrap().metadata.limits.max_work,
            2_000_000_000
        );
        if expected == 0 {
            assert_eq!(
                coverage.detail,
                evm_abstract_protocol::HybridDetailState::Disabled
            );
            assert!(report.native.is_none());
            assert_eq!(coverage.work.preparation + coverage.work.detail, 0);
        } else {
            let native = report.native.as_ref().unwrap();
            assert!(
                native
                    .cfg
                    .iter()
                    .any(|state| state.executed_pcs == [0, 2, 4, 5])
            );
            assert!(!native.ssa.blocks.is_empty());
        }
    }
    pool.shutdown().unwrap();
}
