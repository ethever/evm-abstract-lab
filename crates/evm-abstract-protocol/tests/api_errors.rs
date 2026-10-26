//! Error envelopes retain every typed field through payload compaction.
//! Validation/RPC fixtures were recorded at b629ffe00443d993db1d4b36dc1c1753bbf968c9;
//! export fixtures follow wire schema 12.
use evm_abstract_protocol::*;

#[test]
fn api_errors_stay_below_the_large_result_error_threshold() {
    // Check an upper bound, not an ABI-specific discriminant or field layout.
    for (name, size) in [
        ("ApiError", std::mem::size_of::<ApiError>()),
        ("ErrorDetails", std::mem::size_of::<ErrorDetails>()),
    ] {
        assert!(size < 128, "{name} occupies {size} bytes");
    }
}

#[test]
fn execution_and_work_failures_keep_their_typed_wire_categories() {
    for details in [
        ErrorDetails::Execution(ExecutionFailureKind::Protocol),
        ErrorDetails::Execution(ExecutionFailureKind::Initialization),
        ErrorDetails::Execution(ExecutionFailureKind::Refinement),
        ErrorDetails::Execution(ExecutionFailureKind::Unfinished),
        ErrorDetails::Ssa(SsaFailure::Work),
        ErrorDetails::Session(Box::new(SessionFailure::Work)),
    ] {
        let error = ApiError {
            code: ApiErrorCode::Internal,
            message: "native contextual diagnostic".into(),
            details: details.clone(),
        };
        let encoded = serde_json::to_value(&error).unwrap();
        assert_eq!(encoded["message"], error.message);
        assert!(encoded["details"].get("Task").is_none());
        assert!(encoded["details"].get("Worker").is_none());
        assert!(encoded["details"].get("Transport").is_none());
        let reply = AnalyzeReply { result: Err(error) };
        assert_eq!(
            serde_json::from_slice::<AnalyzeReply>(&serde_json::to_vec(&reply).unwrap()).unwrap(),
            reply
        );
        if let ErrorDetails::Execution(kind) = details {
            assert_eq!(
                encoded["details"]["Execution"],
                serde_json::to_value(kind).unwrap()
            );
        }
    }
    for invalid in [
        r#"{"Execution":"Unknown"}"#,
        r#"{"Execution":{"Protocol":null}}"#,
        r#"{"Execution":null}"#,
        r#"{"Execution":"Protocol","Execution":"Refinement"}"#,
        r#"{"Ssa":{"Work":1}}"#,
        r#"{"Session":{"Work":{}}}"#,
    ] {
        assert!(
            serde_json::from_str::<ErrorDetails>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn session_refinement_refusals_keep_the_location_and_typed_source() {
    let location = InitialFactLocation::Storage(StorageLocation {
        address: format!("0x{}", "11".repeat(20)),
        slot: format!("0x{}", "22".repeat(32)),
    });
    for source in [
        FactFailure::Capacity(FactCapacityFailure {
            max_atoms: 64,
            required: 65,
        }),
        FactFailure::UnsupportedOperation(0xfe),
        FactFailure::OriginContradiction(u32::MAX),
    ] {
        let failure = SessionFailure::Refinement(SessionRefinementFailure {
            location: location.clone(),
            source: source.clone(),
        });
        let encoded = serde_json::to_value(&failure).unwrap();
        assert_eq!(
            encoded["Refinement"]["source"],
            serde_json::to_value(source).unwrap()
        );
        assert_ne!(
            encoded,
            serde_json::to_value(SessionFailure::FactConflict(location.clone())).unwrap()
        );
        assert_eq!(
            serde_json::from_value::<SessionFailure>(encoded.clone()).unwrap(),
            failure
        );
        for field in ["location", "source"] {
            let mut missing = encoded.clone();
            missing["Refinement"].as_object_mut().unwrap().remove(field);
            let error = serde_json::from_value::<SessionFailure>(missing).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("missing field: {field}"))
            );
        }
        let mut unknown = encoded;
        unknown["Refinement"]["reason"] = serde_json::json!("unavailable");
        let error = serde_json::from_value::<SessionFailure>(unknown).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown SessionRefinementFailure field: reason")
        );
    }
    for invalid in [
        r#"{"Refinement":"unavailable"}"#,
        r#"{"Refinement":{"location":{"Balance":"0x1"},"source":"Unsupported"}}"#,
    ] {
        assert!(serde_json::from_str::<SessionFailure>(invalid).is_err());
    }
}

#[test]
fn postprocessing_rejections_preserve_allowance_and_usage_as_u64() {
    let failure = ReportValidationFailure::Postprocessing(PostprocessingWorkFailure {
        max_work: 9_007_199_254_740_993,
        work: u64::MAX,
    });
    let encoded = serde_json::to_string(&failure).unwrap();
    assert_eq!(
        serde_json::from_str::<ReportValidationFailure>(&encoded).unwrap(),
        failure
    );
    for (field, count) in [("max_work", 9_007_199_254_740_993), ("work", u64::MAX)] {
        let mut missing = serde_json::to_value(&failure).unwrap();
        missing["Postprocessing"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let duplicate = encoded.replace(
            &format!("\"{field}\":{count}"),
            &format!("\"{field}\":{count},\"{field}\":{count}"),
        );
        let error = serde_json::from_value::<ReportValidationFailure>(missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field: {field}"))
        );
        let error = serde_json::from_str::<ReportValidationFailure>(&duplicate).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("duplicate field: {field}"))
        );
        assert!(failure.to_string().contains(&count.to_string()));
    }
}

fn assert_frozen_wire(error: ApiError, error_json: &str, reply_json: &str) {
    assert_eq!(serde_json::from_str::<ApiError>(error_json).unwrap(), error);
    assert_eq!(serde_json::to_string(&error).unwrap(), error_json);
    let reply = AnalyzeReply { result: Err(error) };
    assert_eq!(
        serde_json::from_str::<AnalyzeReply>(reply_json).unwrap(),
        reply
    );
    assert_eq!(serde_json::to_string(&reply).unwrap(), reply_json);
}

#[test]
fn export_error_preserves_every_policy_usage_and_written_field_on_schema_12_wire() {
    assert_frozen_wire(
        ApiError {
            code: ApiErrorCode::Export,
            message: "report export ExactBytes boundary (required 8193)".into(),
            details: ErrorDetails::Export(Box::new(ExportFailure {
                kind: ExportFailureKind::ExactBytes,
                required: 8193,
                limits: ExportLimits {
                    max_nodes: 64,
                    max_depth: 16,
                    max_exact_bytes: 4096,
                },
                usage: ExportUsage {
                    nodes: 7,
                    exact_bytes: 31,
                    work: 127,
                },
                written: None,
            })),
        },
        include_str!("fixtures/api-errors/export-api-error.json"),
        include_str!("fixtures/api-errors/export-analyze-reply.json"),
    );
}

#[test]
fn export_cancellation_retains_written_bytes_beyond_the_former_output_limit() {
    let mut error: ApiError =
        serde_json::from_str(include_str!("fixtures/api-errors/export-api-error.json")).unwrap();
    let written = 64 * 1024 * 1024 + 8192;
    error.code = ApiErrorCode::Cancelled;
    error.message = format!("native JSON export cancelled after {written} bytes");
    let ErrorDetails::Export(detail) = &mut error.details else {
        panic!("expected export failure details");
    };
    detail.kind = ExportFailureKind::Cancelled;
    detail.required = written as u64;
    detail.written = Some(written);
    let reply = AnalyzeReply { result: Err(error) };
    assert_eq!(
        serde_json::from_str::<AnalyzeReply>(&serde_json::to_string(&reply).unwrap()).unwrap(),
        reply
    );
}

#[test]
fn validation_error_preserves_exact_input_coordinates_on_the_old_wire() {
    assert_frozen_wire(
        ApiError {
            code: ApiErrorCode::InvalidRequest,
            message: "invalid address".into(),
            details: ErrorDetails::Validation(ValidationFailure {
                kind: ValidationErrorKind::Address,
                field: Some("input.address".into()),
                value: Some("0x123".into()),
            }),
        },
        include_str!("fixtures/api-errors/validation-api-error.json"),
        include_str!("fixtures/api-errors/validation-analyze-reply.json"),
    );
}

#[test]
fn rpc_error_retains_the_exact_nested_world_source_on_the_old_wire() {
    assert_frozen_wire(
        ApiError {
            code: ApiErrorCode::Rpc,
            message: "snapshot code hash disagrees".into(),
            details: ErrorDetails::Rpc(Box::new(RpcFailure {
                kind: RpcFailureKind::World,
                chain_id: Some("0x1".into()),
                block_hash: Some(format!("0x{}", "11".repeat(32))),
                method: "eth_getCode".into(),
                account: Some(format!("0x{}", "22".repeat(20))),
                slot: None,
                resource: None,
                limit: None,
                http_status: Some(200),
                rpc_code: None,
                json_line: None,
                json_column: None,
                cause: Some(RpcFailureCause::World(WorldFailure::CodeHash(
                    CodeHashMismatch {
                        address: format!("0x{}", "22".repeat(20)),
                        expected: format!("0x{}", "33".repeat(32)),
                        observed: format!("0x{}", "44".repeat(32)),
                    },
                ))),
                message: "code hash mismatch".into(),
            })),
        },
        include_str!("fixtures/api-errors/rpc-api-error.json"),
        include_str!("fixtures/api-errors/rpc-analyze-reply.json"),
    );
}

#[test]
fn export_payload_keeps_strict_typed_field_validation() {
    let json = include_str!("fixtures/api-errors/export-api-error.json");
    for invalid in [
        json.replace("\"written\":null", "\"written\":null,\"written\":null"),
        json.replace(",\"written\":null", ""),
        json.replace("\"max_nodes\":64", "\"max_nodes\":64,\"unknown_limit\":1"),
        json.replace("\"max_nodes\":64", "\"max_nodes\":64,\"max_work\":16384"),
        json.replace(
            "\"max_nodes\":64",
            "\"max_nodes\":64,\"max_output_bytes\":67108864",
        ),
        json.replace("\"ExactBytes\"", "\"Unpaid\""),
        json.replace("\"ExactBytes\"", "\"OutputBytes\""),
        json.replace("\"ExactBytes\"", "\"UnknownBoundary\""),
    ] {
        assert!(
            serde_json::from_str::<ApiError>(&invalid).is_err(),
            "{invalid}"
        );
        let reply = format!("{{\"result\":{{\"Err\":{invalid}}}}}");
        assert!(
            serde_json::from_str::<AnalyzeReply>(&reply).is_err(),
            "{reply}"
        );
    }
    for kind in [ExportFailureKind::Work, ExportFailureKind::Numeric] {
        let mut error: ApiError = serde_json::from_str(json).unwrap();
        let ErrorDetails::Export(failure) = &mut error.details else {
            unreachable!()
        };
        failure.kind = kind;
        assert_eq!(
            serde_json::from_slice::<ApiError>(&serde_json::to_vec(&error).unwrap()).unwrap(),
            error
        );
    }
}
