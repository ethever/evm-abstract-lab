//! Shared typed contracts work without the native engine or JSON value erasure.
use evm_abstract_protocol::*;
fn roundtrip<
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
>(
    value: T,
) {
    assert_eq!(
        serde_json::from_slice::<T>(&serde_json::to_vec(&value).unwrap()).unwrap(),
        value
    );
}
#[test]
fn bytecode_and_rpc_requests_roundtrip() {
    roundtrip(AnalyzeRequest::default());
    let mut request = AnalyzeRequest {
        input: AnalysisInput::Rpc(RpcInput {
            provider_id: "mainnet".into(),
            address: format!("0x{}", "11".repeat(20)),
            block: BlockSelector::Number(42),
            accounts: vec![AccountQuery {
                address: format!("0x{}", "22".repeat(20)),
                slots: vec!["0x123".into()],
            }],
        }),
        ..AnalyzeRequest::default()
    };
    request.environment.calldata = CalldataInput::Exact("0x0102".into());
    request.environment.origin = Some(AddressSetting::Unknown);
    request.environment.number = Some("0x42".into());
    roundtrip(request);
}
#[test]
fn errors_and_job_states_roundtrip_without_invalid_optional_combinations() {
    let error = ApiError {
        code: ApiErrorCode::InvalidBytecode,
        message: "invalid hex digit".into(),
        details: ErrorDetails::Bytecode(BytecodeFailure::InvalidHex(HexDigitFailure {
            index: 2,
            character: 'g',
        })),
    };
    roundtrip(AnalyzeReply {
        result: Err(error.clone()),
    });
    for state in [
        JobState::Queued,
        JobState::Running,
        JobState::Cancelling,
        JobState::Cancelled,
        JobState::Completed,
        JobState::Failed(error),
    ] {
        roundtrip(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(9_007_199_254_740_993),
                state,
                progress: AnalysisProgress::default(),
            }),
        });
    }
}
#[test]
fn misspelled_duplicate_missing_unknown_and_multi_variant_inputs_are_rejected() {
    let valid = serde_json::to_string(&AnalyzeRequest::default()).unwrap();
    for invalid in [
        valid.replace("\"Osaka\"", "\"future\""),
        valid.replacen("\"fork\":", "\"fork\":\"Osaka\",\"fork\":", 1),
        valid.replacen("\"fork\":\"Osaka\",", "", 1),
        valid.replace("max_constants", "max_constant"),
        valid.replacen("\"Bytecode\":", "\"UnknownInput\":", 1),
    ] {
        assert!(
            serde_json::from_str::<AnalyzeRequest>(&invalid).is_err(),
            "{invalid}"
        );
    }
    for invalid in [
        r#"{"Latest":null,"Number":1}"#,
        r#"{"Number":1,"Number":2}"#,
        r#"{"Future":1}"#,
        r#"{}"#,
    ] {
        assert!(serde_json::from_str::<BlockSelector>(invalid).is_err());
    }
}
#[test]
fn nullable_fields_remain_required_and_duplicate_null_is_rejected() {
    let valid = r#"{"from":null,"pc":null,"kind":"Memory","reason":"Memory","detail":"bounded"}"#;
    let decoded: Frontier = serde_json::from_str(valid).unwrap();
    assert_eq!(decoded.from, None);
    for invalid in [
        valid.replace("\"from\":null,", "\"from\":null,\"from\":null,"),
        valid.replace("\"pc\":null,", ""),
    ] {
        assert!(serde_json::from_str::<Frontier>(&invalid).is_err());
    }
}

#[test]
fn relational_failures_preserve_fact_coordinates_and_solver_outcomes() {
    for cause in [
        QueryBoundary::ScalarFactError(FactFailure::InvalidOperation(FactOperationFailure {
            opcode: 0x01,
            expected: 2,
            actual: 1,
        })),
        QueryBoundary::ScalarFactError(FactFailure::Capacity(FactCapacityFailure {
            max_atoms: 64,
            required: 65,
        })),
        QueryBoundary::ScalarFactError(FactFailure::OriginContradiction(7)),
        QueryBoundary::SolverUnknown(SolverUnknown {
            provider: SmtProvider::Z3,
            reason: "theory limitation".into(),
        }),
        QueryBoundary::SolverError(SolverFailure {
            provider: SmtProvider::Cvc5,
            message: "native binding failure".into(),
        }),
    ] {
        roundtrip(Frontier {
            from: Some(12),
            pc: Some(34),
            kind: FrontierKind::Relations,
            reason: FrontierDetails::Relations(cause),
            detail: "supplemental diagnostic".into(),
        });
    }
}

#[test]
fn transport_diagnostic_roundtrip_preserves_reason_beside_classification() {
    roundtrip(RpcFailureCause::Transport(RpcTransportFailure {
        timeout: false,
        connect: false,
        builder: true,
        request: false,
        body: false,
        decode: false,
        redirect: false,
        native_diagnostic: "builder error: no native root certificates were found".into(),
    }));
}

#[test]
fn rpc_catalogue_includes_url_values_and_selection_remains_id_based() {
    roundtrip(RpcProvidersReply {
        result: Ok(vec![RpcProvider {
            id: "mainnet".into(),
            name: "Ethereum mainnet".into(),
            endpoint: "https://example.com/rpc?key=value".into(),
        }]),
    });
    roundtrip(RpcProvidersReply {
        result: Ok(Vec::new()),
    });
    let input = RpcInput {
        provider_id: "mainnet".into(),
        address: format!("0x{}", "11".repeat(20)),
        block: BlockSelector::Latest,
        accounts: Vec::new(),
    };
    let valid = serde_json::to_string(&input).unwrap();
    assert!(!valid.contains("endpoint"));
    for invalid in [
        valid.replace("provider_id", "endpoint"),
        valid.replacen("{", r#"{"endpoint":"http://user:secret@example.com", "#, 1),
        valid.replace(r#""provider_id":"mainnet","#, ""),
        valid.replace(
            r#""provider_id":"mainnet""#,
            r#""provider_id":"mainnet","provider_id":"other""#,
        ),
    ] {
        assert!(
            serde_json::from_str::<RpcInput>(&invalid).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(SCHEMA_VERSION, 4);
}
