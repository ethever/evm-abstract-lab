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
fn limits_roundtrip_above_old_caps_without_losing_large_integer_precision() {
    let count = 9_007_199_254_740_993_u64;
    let limits = AnalysisLimits {
        max_states: count,
        max_transfers: count,
        context_depth: count,
        max_constants: count,
        max_work: count,
        max_call_depth: count,
        max_memory_bytes: count,
        use_summaries: false,
        domain_profile: DomainProfile::ConstantsOnly,
        reduction_rounds: count,
        max_facts: count,
        relations_enabled: false,
        max_constraints: count,
        max_expression_nodes: count,
        max_expression_depth: count,
        smt_provider: SmtProvider::Cvc5,
        smt_rlimit: u32::MAX,
        rpc_max_accounts: count,
        rpc_max_requests: count,
        rpc_max_response_bytes: count,
        rpc_timeout_ms: count,
    };
    roundtrip(AnalyzeRequest {
        limits,
        ..AnalyzeRequest::default()
    });
    let defaults = AnalysisLimits::default();
    assert_eq!(defaults.context_depth, 128);
    assert_eq!(defaults.max_work, 1_000_000_000_000);
    assert!(defaults.max_work > u64::from(u32::MAX));
    roundtrip(defaults);
}

#[test]
fn cumulative_counters_preserve_u64_values_above_wasm_and_javascript_integer_ranges() {
    for count in [u64::from(u32::MAX) + 1, 9_007_199_254_740_993, u64::MAX] {
        let progress = AnalysisProgress {
            phase: AnalysisPhase::Analyzing,
            transfers: count,
            work: count,
            ..AnalysisProgress::default()
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains(&format!("\"transfers\":{count}")));
        assert!(json.contains(&format!("\"work\":{count}")));
        roundtrip(progress);
    }
}

#[test]
fn monotonic_timings_preserve_nanoseconds_without_javascript_integer_rounding() {
    for count in [9_007_199_254_740_993, u64::MAX] {
        let timings = AnalysisTimings {
            queue_ns: count,
            processing_ns: count,
            validating_ns: count,
            pinning_ns: count,
            acquiring_ns: count,
            analyzing_ns: count,
            building_ssa_ns: count,
            projecting_ns: count,
        };
        let encoded = serde_json::to_string(&timings).unwrap();
        assert!(encoded.contains(&format!("\"processing_ns\":{count}")));
        assert!(
            serde_json::from_str::<AnalysisTimings>(
                &encoded.replace(&format!("\"queue_ns\":{count},"), "")
            )
            .is_err()
        );
        roundtrip(timings);
    }
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
                timings: AnalysisTimings::default(),
            }),
        });
    }
}
#[test]
fn fingerprint_failures_keep_typed_causes_and_required_encoding_coordinates() {
    for (kind, code, line, column) in [
        (
            FingerprintFailureKind::Cancelled,
            ApiErrorCode::Cancelled,
            None,
            None,
        ),
        (
            FingerprintFailureKind::Encoding,
            ApiErrorCode::Export,
            Some(0),
            Some(0),
        ),
        (
            FingerprintFailureKind::LengthOverflow,
            ApiErrorCode::Export,
            None,
            None,
        ),
    ] {
        let error = ApiError {
            code,
            message: "snapshot fingerprint unavailable".into(),
            details: ErrorDetails::Fingerprint(FingerprintFailure { kind, line, column }),
        };
        let encoded = serde_json::to_string(&error).unwrap();
        for missing in ["line", "column"] {
            let mut invalid: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            invalid["details"]["Fingerprint"]
                .as_object_mut()
                .unwrap()
                .remove(missing);
            assert!(serde_json::from_value::<ApiError>(invalid).is_err());
        }
        roundtrip(AnalyzeReply { result: Err(error) });
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
    let valid = r#"{"from":null,"pc":null,"kind":"Memory","reason":"Memory","detail":"bounded","range":null,"payload":null}"#;
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
            range: None,
            payload: None,
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
    assert_eq!(SCHEMA_VERSION, 9);
}

#[test]
fn invalid_opcode_evidence_roundtrips_instruction_and_delegation_sources() {
    for evidence in [
        InvalidOpcodeDiagnostic::Instruction(InvalidInstruction {
            opcode: 0xfe,
            mnemonic: Some("INVALID".into()),
            reason: InvalidOpcodeReason::ExplicitInvalid,
        }),
        InvalidOpcodeDiagnostic::Instruction(InvalidInstruction {
            opcode: 0xef,
            mnemonic: None,
            reason: InvalidOpcodeReason::UndefinedOpcode,
        }),
        InvalidOpcodeDiagnostic::Instruction(InvalidInstruction {
            opcode: 0x1e,
            mnemonic: Some("CLZ".into()),
            reason: InvalidOpcodeReason::UnavailableInFork(Fork::Cancun),
        }),
        InvalidOpcodeDiagnostic::DelegationEntry(InvalidDelegationEntry {
            opcode: Some(0xef),
            detail: "nested delegation indicator cannot be executed".into(),
            byte_unavailable: None,
        }),
        InvalidOpcodeDiagnostic::DelegationEntry(InvalidDelegationEntry {
            opcode: None,
            detail: "delegation entry rejected".into(),
            byte_unavailable: Some("active account code is unavailable".into()),
        }),
    ] {
        roundtrip(Diagnostic {
            precision: None,
            state: 123,
            pc: 0,
            kind: DiagnosticKind::InvalidOpcode,
            reduction: None,
            invalid_opcode: Some(evidence),
            detail: "exceptional halt".into(),
        });
    }
    roundtrip(Diagnostic {
        precision: None,
        state: 456,
        pc: 1,
        kind: DiagnosticKind::InvalidJump,
        reduction: None,
        invalid_opcode: None,
        detail: "InvalidJump".into(),
    });
}

#[test]
fn invalid_instruction_wire_requires_original_byte_and_typed_cause() {
    let valid = r#"{"Instruction":{"opcode":239,"mnemonic":null,"reason":"UndefinedOpcode"}}"#;
    assert_eq!(
        serde_json::from_str::<InvalidOpcodeDiagnostic>(valid).unwrap(),
        InvalidOpcodeDiagnostic::Instruction(InvalidInstruction {
            opcode: 0xef,
            mnemonic: None,
            reason: InvalidOpcodeReason::UndefinedOpcode,
        })
    );
    for invalid in [
        valid.replace("\"opcode\":239,", ""),
        valid.replace("\"opcode\":239", "\"opcode\":null"),
        valid.replace("\"opcode\":239", "\"opcode\":256"),
        valid.replace("UndefinedOpcode", "UnknownReason"),
        valid.replace("\"mnemonic\":null,", ""),
        valid.replace("\"opcode\":239,", "\"opcode\":239,\"opcode\":254,"),
    ] {
        assert!(
            serde_json::from_str::<InvalidOpcodeDiagnostic>(&invalid).is_err(),
            "{invalid}"
        );
    }
    let diagnostic = r#"{"state":123,"pc":0,"kind":"InvalidOpcode","reduction":null,"invalid_opcode":null,"detail":"old category"}"#;
    assert!(
        serde_json::from_str::<Diagnostic>(&diagnostic.replace("\"invalid_opcode\":null,", ""))
            .is_err(),
        "schema 7 diagnostics are rejected by the schema 8 decoder"
    );
}

#[test]
fn mixed_graph_checks_sorts_and_backward_dependencies() {
    let graph = SymbolicGraph {
        nodes: vec![
            Expression::BytesZero,
            Expression::Constant("0x0".into()),
            Expression::View(ByteView {
                state: ByteStateRef { node: 0 },
                offset: ExpressionRef { node: 1 },
                length: ExpressionRef { node: 1 },
            }),
            Expression::WordRead(WordRead {
                view: ByteViewRef { node: 2 },
                offset: ExpressionRef { node: 1 },
            }),
        ],
    };
    graph.validate().unwrap();
    roundtrip(graph.clone());
    let mut wrong = graph.clone();
    wrong.nodes[3] = Expression::WordRead(WordRead {
        view: ByteViewRef { node: 0 },
        offset: ExpressionRef { node: 1 },
    });
    assert_eq!(
        wrong.validate().unwrap_err().actual,
        Some(SymbolicSort::Bytes)
    );
    wrong.nodes[3] = Expression::WordRead(WordRead {
        view: ByteViewRef { node: 3 },
        offset: ExpressionRef { node: 1 },
    });
    assert!(wrong.validate().unwrap_err().forward);
    wrong.nodes[3] = Expression::WordRead(WordRead {
        view: ByteViewRef { node: usize::MAX },
        offset: ExpressionRef { node: 1 },
    });
    assert_eq!(wrong.validate().unwrap_err().actual, None);
}
