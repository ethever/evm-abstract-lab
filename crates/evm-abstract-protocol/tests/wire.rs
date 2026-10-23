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
        root_terminal_coverage: true,
        numeric_model: NumericModel::FiniteSet,
        reduction_rounds: count,
        max_facts: count,
        relations_enabled: false,
        max_constraints: count,
        max_expression_nodes: count,
        max_expression_depth: count,
        rpc_max_accounts: count,
        rpc_max_requests: count,
        rpc_max_response_bytes: count,
        rpc_timeout_ms: count,
    };
    let encoded = serde_json::to_value(&limits).unwrap();
    assert_eq!(encoded["max_work"].as_u64(), Some(9_007_199_254_740_993));
    roundtrip(AnalyzeRequest {
        limits,
        ..AnalyzeRequest::default()
    });
}

#[test]
fn consumer_defaults_enable_fast_conservative_coverage() {
    let defaults = AnalysisLimits::default();
    assert!(defaults.root_terminal_coverage);
    assert_eq!(defaults.context_depth, 128);
    assert_eq!(defaults.max_work, 2_000_000_000);
    assert_eq!(defaults.numeric_model, NumericModel::Product);
    assert_eq!(AnalyzeRequest::default().limits, defaults);
    roundtrip(defaults);
}

#[test]
fn numeric_models_are_explicit_and_legacy_selectors_are_rejected() {
    for model in [NumericModel::Product, NumericModel::FiniteSet] {
        let mut request = AnalyzeRequest::default();
        request.limits.numeric_model = model;
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["limits"]["numeric_model"], format!("{model:?}"));
        assert!(encoded["limits"].get("domain_profile").is_none());
        roundtrip(request);
    }
    let encoded = serde_json::to_value(AnalyzeRequest::default()).unwrap();
    for keep_model in [true, false] {
        let mut legacy = encoded.clone();
        let limits = legacy["limits"].as_object_mut().unwrap();
        limits.insert("domain_profile".into(), "ConstantsOnly".into());
        if !keep_model {
            limits.remove("numeric_model");
        }
        let error = serde_json::from_value::<AnalyzeRequest>(legacy).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown AnalysisLimits field: domain_profile"),
            "{error}"
        );
    }
    for invalid in [
        serde_json::json!("ConstantsOnly"),
        serde_json::json!("constants-only"),
        serde_json::json!("product"),
        serde_json::json!("finite-set"),
        serde_json::Value::Null,
        serde_json::json!(0),
        serde_json::json!({"FiniteSet": null}),
    ] {
        let mut request = encoded.clone();
        request["limits"]["numeric_model"] = invalid;
        assert!(serde_json::from_value::<AnalyzeRequest>(request).is_err());
    }
    let encoded = serde_json::to_string(&AnalysisLimits::default()).unwrap();
    let duplicate = encoded.replacen('{', r#"{"numeric_model":"FiniteSet","#, 1);
    let error = serde_json::from_str::<AnalysisLimits>(&duplicate).unwrap_err();
    assert!(
        error.to_string().contains("duplicate field: numeric_model"),
        "{error}"
    );
}

#[test]
fn all_request_and_limit_fields_are_required_on_the_wire() {
    let encoded = serde_json::to_value(AnalyzeRequest::default()).unwrap();
    for field in encoded.as_object().unwrap().keys() {
        let mut missing = encoded.clone();
        assert!(missing.as_object_mut().unwrap().remove(field).is_some());
        let error = serde_json::from_value::<AnalyzeRequest>(missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field: {field}")),
            "{error}"
        );
    }
    for field in encoded["limits"].as_object().unwrap().keys() {
        let mut missing = encoded.clone();
        assert!(
            missing["limits"]
                .as_object_mut()
                .unwrap()
                .remove(field)
                .is_some()
        );
        let error = serde_json::from_value::<AnalyzeRequest>(missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field: {field}")),
            "{error}"
        );
    }
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
fn export_work_remains_diagnostic_above_the_former_hard_cap() {
    for work in [
        64_003_705,
        u64::from(u32::MAX) + 1,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let usage = ExportUsage {
            nodes: 7,
            exact_bytes: 31,
            work,
        };
        let json = serde_json::to_string(&usage).unwrap();
        assert!(json.contains(&format!("\"work\":{work}")));
        roundtrip(usage);
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
fn relational_boundaries_preserve_local_causes_and_fact_coordinates() {
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
        QueryBoundary::Cancelled,
        QueryBoundary::Disabled,
        QueryBoundary::Configuration,
        QueryBoundary::ConstraintLimit,
        QueryBoundary::ScalarFactLimit,
        QueryBoundary::ExpressionLimit,
        QueryBoundary::ScalarFactError(FactFailure::UnsupportedOperation(0xfe)),
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
    assert_eq!(SCHEMA_VERSION, 19);
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
    let diagnostic = r#"{"state":123,"pc":0,"kind":"InvalidOpcode","reduction":null,"precision":null,"invalid_opcode":null,"detail":"old category"}"#;
    serde_json::from_str::<Diagnostic>(diagnostic).unwrap();
    assert!(
        serde_json::from_str::<Diagnostic>(&diagnostic.replace("\"invalid_opcode\":null,", ""))
            .is_err(),
        "diagnostics missing typed opcode evidence are rejected"
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

#[test]
fn observed_binding_edges_include_numeric_references_in_stored_order() {
    let mut numeric = touch_snapshot(None, None).accounts[0].balance.clone();
    numeric.expression = Some(ExpressionRef { node: 1 });
    let binding = SymbolicBinding {
        index: "0x2".into(),
        value: ExpressionRef { node: 2 },
        numeric,
    };
    let source = SymbolicSource {
        token: ExpressionRef { node: 0 },
        namespace: "0x123".into(),
    };
    let mut graph = SymbolicGraph {
        nodes: vec![
            Expression::Fresh(0),
            Expression::Constant("0x20".into()),
            Expression::Fresh(1),
            Expression::BytesZero,
            Expression::BytesInput(BytesInput {
                source: source.clone(),
                length: ExpressionRef { node: 1 },
                bindings: vec![binding.clone()],
            }),
            Expression::SlotsInput(SlotsInput {
                source,
                bindings: vec![binding],
            }),
            Expression::View(ByteView {
                state: ByteStateRef { node: 3 },
                offset: ExpressionRef { node: 0 },
                length: ExpressionRef { node: 1 },
            }),
            Expression::Copy(ByteCopy {
                previous: ByteStateRef { node: 4 },
                destination: ExpressionRef { node: 2 },
                source: ByteViewRef { node: 6 },
                length: ExpressionRef { node: 1 },
            }),
        ],
    };
    let word = |node| SymbolicReference {
        sort: SymbolicSort::Word,
        node,
    };
    assert_eq!(
        graph.nodes[4].references(),
        vec![word(0), word(1), word(2), word(1)]
    );
    assert_eq!(graph.nodes[5].references(), vec![word(0), word(2), word(1)]);
    assert_eq!(
        graph.nodes[7].references(),
        vec![
            SymbolicReference {
                sort: SymbolicSort::Bytes,
                node: 4
            },
            word(2),
            SymbolicReference {
                sort: SymbolicSort::View,
                node: 6
            },
            word(1),
        ]
    );
    graph.validate().unwrap();
    roundtrip(graph.clone());
    let Expression::BytesInput(input) = &mut graph.nodes[4] else {
        unreachable!()
    };
    input.bindings[0].numeric.expression = Some(ExpressionRef { node: 3 });
    let error = graph.validate().unwrap_err();
    assert_eq!(
        (
            error.owner,
            error.node,
            error.expected,
            error.actual,
            error.forward
        ),
        (
            Some(4),
            3,
            SymbolicSort::Word,
            Some(SymbolicSort::Bytes),
            false
        )
    );
    let Expression::BytesInput(input) = &mut graph.nodes[4] else {
        unreachable!()
    };
    input.bindings[0].numeric.expression = Some(ExpressionRef { node: 4 });
    assert!(graph.validate().unwrap_err().forward);
    let mut missing = serde_json::to_value(&graph.nodes[4]).unwrap();
    missing["BytesInput"]
        .as_object_mut()
        .unwrap()
        .remove("length");
    assert!(
        serde_json::from_value::<Expression>(missing)
            .unwrap_err()
            .to_string()
            .contains("missing field: length")
    );
    let duplicate = r#"{"WriteByte":{"previous":{"node":0},"offset":{"node":1},"value":{"node":2},"value":{"node":3}}}"#;
    assert!(
        serde_json::from_str::<Expression>(duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate field: value")
    );
}

#[test]
fn wide_binding_validation_streams_borrowed_edges_and_keeps_the_first_failure() {
    let mut numeric = touch_snapshot(None, None).accounts[0].balance.clone();
    numeric.expression = Some(ExpressionRef { node: 0 });
    let mut graph = SymbolicGraph {
        nodes: vec![
            Expression::Fresh(0),
            Expression::Fresh(1),
            Expression::BytesZero,
            Expression::BytesInput(BytesInput {
                source: SymbolicSource {
                    token: ExpressionRef { node: 0 },
                    namespace: "0x123".into(),
                },
                length: ExpressionRef { node: 1 },
                bindings: vec![
                    SymbolicBinding {
                        index: "0x0".into(),
                        value: ExpressionRef { node: 1 },
                        numeric,
                    };
                    4096
                ],
            }),
        ],
    };
    graph.validate().unwrap();
    let Expression::BytesInput(input) = &graph.nodes[3] else {
        unreachable!()
    };
    let mut visits = 0;
    let stopped = graph.nodes[3].structure().unwrap().try_visit_edges(|edge| {
        visits += 1;
        let symbolic_structure::Edge::Word(expression) = edge else {
            panic!("word operands")
        };
        if visits == 3 {
            assert!(std::ptr::eq(expression, &input.bindings[0].value));
        }
        if visits == 4 {
            assert!(std::ptr::eq(
                expression,
                input.bindings[0].numeric.expression.as_ref().unwrap()
            ));
            return Err("stop at first numeric edge");
        }
        Ok(())
    });
    assert_eq!(stopped, Err("stop at first numeric edge"));
    assert_eq!(visits, 4);

    let Expression::BytesInput(input) = &mut graph.nodes[3] else {
        unreachable!()
    };
    input.bindings[0].numeric.expression = Some(ExpressionRef { node: 2 });
    input.bindings.last_mut().unwrap().value.node = usize::MAX;
    let error = graph.validate().unwrap_err();
    assert_eq!(
        (
            error.owner,
            error.node,
            error.expected,
            error.actual,
            error.forward
        ),
        (
            Some(3),
            2,
            SymbolicSort::Word,
            Some(SymbolicSort::Bytes),
            false
        )
    );
}

#[test]
fn path_reference_stream_preserves_the_first_invalid_operand_and_sort() {
    let word = |node| ExpressionRef { node };
    let path = |node| PathRef { node };
    let mut graph = SymbolicGraph {
        nodes: vec![
            Expression::Fresh(0),
            Expression::PathTrue,
            Expression::PathAtoms(vec![PathConstraint::Equal(PathEquality {
                left: word(0),
                right: word(0),
            })]),
            Expression::PathIte(PathIte {
                selector: word(0),
                then_path: path(2),
                else_path: path(1),
            }),
            Expression::PathAnd(vec![path(3), path(1)]),
        ],
    };
    graph.validate().unwrap();
    assert_eq!(
        graph.nodes[3].references(),
        vec![
            SymbolicReference {
                sort: SymbolicSort::Word,
                node: 0
            },
            SymbolicReference {
                sort: SymbolicSort::Path,
                node: 2
            },
            SymbolicReference {
                sort: SymbolicSort::Path,
                node: 1
            },
        ]
    );
    for (invalid, node, expected, actual) in [
        (
            Expression::PathAtoms(vec![PathConstraint::Equal(PathEquality {
                left: word(1),
                right: word(usize::MAX),
            })]),
            1,
            SymbolicSort::Word,
            SymbolicSort::Path,
        ),
        (
            Expression::PathIte(PathIte {
                selector: word(0),
                then_path: path(0),
                else_path: path(usize::MAX),
            }),
            0,
            SymbolicSort::Path,
            SymbolicSort::Word,
        ),
        (
            Expression::PathAnd(vec![path(0), path(usize::MAX)]),
            0,
            SymbolicSort::Path,
            SymbolicSort::Word,
        ),
    ] {
        graph.nodes[2] = invalid;
        let error = graph.validate().unwrap_err();
        assert_eq!(
            (
                error.owner,
                error.node,
                error.expected,
                error.actual,
                error.forward
            ),
            (Some(2), node, expected, Some(actual), false)
        );
    }
}

#[test]
fn procedure_summary_wire_requires_entry_kind_and_nullable_root() {
    for kind in [
        ProcedureSummaryKind::UnknownEnvironment,
        ProcedureSummaryKind::KnownRecursion,
        ProcedureSummaryKind::KnownCall,
    ] {
        let summary = AbstractSummary {
            source_state: 7,
            source_current: false,
            kind,
            root_procedure: None,
            closed: false,
            procedures: vec![],
            regional_cfg: RegionalCfg::default(),
            state_count: 0,
            transfers: 0,
            propagations: 0,
            reused: 0,
        };
        let json = serde_json::to_value(&summary).unwrap();
        let roundtrip: AbstractSummary = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(roundtrip, summary);
        for field in ["kind", "root_procedure", "regional_cfg", "source_current"] {
            let mut missing = json.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<AbstractSummary>(missing).is_err());
        }
        let mut invalid_kind = json;
        invalid_kind["kind"] = serde_json::json!("Runtime");
        assert!(serde_json::from_value::<AbstractSummary>(invalid_kind).is_err());
    }
}

#[test]
fn obsolete_solver_settings_and_causes_are_explicitly_rejected() {
    for (name, value) in [
        ("smt_provider", serde_json::json!("z3")),
        ("smt_rlimit", serde_json::json!(10000000)),
    ] {
        let mut request = serde_json::to_value(AnalyzeRequest::default()).unwrap();
        request["limits"][name] = value;
        let error = serde_json::from_value::<AnalyzeRequest>(request).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("unknown AnalysisLimits field: {name}")),
            "{error}"
        );
    }
    for obsolete in [
        serde_json::json!("ResourceLimit"),
        serde_json::json!("ModelUnavailable"),
        serde_json::json!({"SolverUnknown": {"provider": "Z3", "reason": "unknown"}}),
        serde_json::json!({"SolverError": {"provider": "Cvc5", "message": "failure"}}),
    ] {
        assert!(serde_json::from_value::<QueryBoundary>(obsolete).is_err());
    }
}

#[test]
fn log_source_identity_is_required_and_distinguishes_init_runtime_and_code_hashes() {
    let hash = format!("0x{}", "11".repeat(32));
    let log = LogSnapshot {
        address: format!("0x{}", "22".repeat(20)),
        code_address: format!("0x{}", "33".repeat(20)),
        code_hash: hash.clone(),
        source_kind: LogSourceKind::Init,
        pc: 7,
        topics: vec![],
        data: 0,
    };
    roundtrip(vec![
        log.clone(),
        LogSnapshot {
            source_kind: LogSourceKind::Runtime,
            ..log.clone()
        },
        LogSnapshot {
            code_hash: format!("0x{}", "44".repeat(32)),
            source_kind: LogSourceKind::Runtime,
            ..log.clone()
        },
    ]);
    for field in ["code_hash", "source_kind"] {
        let mut missing = serde_json::to_value(&log).unwrap();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<LogSnapshot>(missing)
                .unwrap_err()
                .to_string()
                .contains(field)
        );
    }
    assert_eq!(serde_json::to_value(&log).unwrap()["code_hash"], hash);
}

fn touch_snapshot(touched: Option<bool>, touched_default: Option<bool>) -> StoreSnapshot {
    let zero = ValueInfo {
        summary: "{0x0}".into(),
        constants: Some(vec!["0x0".into()]),
        known_zero: format!("0x{}", "f".repeat(64)),
        known_one: "0x0".into(),
        nonzero: false,
        unsigned: WordBounds {
            lower: "0x0".into(),
            upper: "0x0".into(),
        },
        signed: WordBounds {
            lower: "0x0".into(),
            upper: "0x0".into(),
        },
        congruence: CongruenceValue::Exact("0x0".into()),
        origins: Some(vec![ValueOrigin::Constant]),
        code_address_role: false,
        identity: None,
        expression: None,
        symbolic_limit: false,
    };
    let account = AccountState {
        address: format!("0x{}", "11".repeat(20)),
        existence: AccountExistence::Present,
        balance: zero.clone(),
        nonce: zero.clone(),
        code_kind: CodeKind::Empty,
        code_hash: None,
        program: None,
        delegation_target: None,
        storage_default: zero.clone(),
        storage_root: SlotStateRef { node: 0 },
        transient_root: SlotStateRef { node: 0 },
        storage: vec![],
        transient_default: zero.clone(),
        transient: vec![],
        created: Some(false),
        touched,
        pending_destruction: Some(false),
    };
    StoreSnapshot {
        storage_default: zero.clone(),
        transient_default: zero.clone(),
        storage_root: SlotStateRef { node: 0 },
        transient_root: SlotStateRef { node: 0 },
        nonce_default: zero.clone(),
        code_default: None,
        existence_default: AccountExistence::Unknown,
        created_default: Some(false),
        pending_destruction_default: Some(false),
        balance_default: zero,
        touched_default,
        accounts: vec![account],
        logs: vec![],
        logs_unknown: false,
        storage_facts: vec![],
        transient_facts: vec![],
    }
}

#[test]
fn transaction_touch_guarantees_and_unlisted_defaults_roundtrip_independently() {
    for touched in [Some(true), Some(false), None] {
        for default in [Some(true), Some(false), None] {
            let snapshot = touch_snapshot(touched, default);
            let encoded = serde_json::to_value(&snapshot).unwrap();
            assert_eq!(
                encoded["accounts"][0]["touched"],
                serde_json::json!(touched)
            );
            assert_eq!(encoded["touched_default"], serde_json::json!(default));
            let decoded: StoreSnapshot = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded.accounts[0].touched, touched);
            assert_eq!(decoded.touched_default, default);
            roundtrip(decoded);
        }
    }
}

#[test]
fn missing_touch_fields_are_rejected_instead_of_defaulting_unknown_to_untouched() {
    let encoded = serde_json::to_value(touch_snapshot(None, None)).unwrap();
    let mut missing_default = encoded.clone();
    missing_default
        .as_object_mut()
        .unwrap()
        .remove("touched_default");
    let error = serde_json::from_value::<StoreSnapshot>(missing_default).unwrap_err();
    assert!(error.to_string().contains("missing field: touched_default"));
    let mut missing_account = encoded;
    missing_account["accounts"][0]
        .as_object_mut()
        .unwrap()
        .remove("touched");
    let error = serde_json::from_value::<StoreSnapshot>(missing_account).unwrap_err();
    assert!(error.to_string().contains("missing field: touched"));
}

#[test]
fn all_global_store_templates_and_defaults_are_required_strict_fields() {
    let snapshot = touch_snapshot(None, None);
    let encoded = serde_json::to_string(&snapshot).unwrap();
    for (field, value) in [
        (
            "storage_root",
            serde_json::to_string(&snapshot.storage_root).unwrap(),
        ),
        (
            "transient_root",
            serde_json::to_string(&snapshot.transient_root).unwrap(),
        ),
        (
            "nonce_default",
            serde_json::to_string(&snapshot.nonce_default).unwrap(),
        ),
        ("code_default", "null".into()),
        ("existence_default", "\"Unknown\"".into()),
        ("created_default", "false".into()),
        ("pending_destruction_default", "false".into()),
    ] {
        let missing = encoded.replacen(&format!("\"{field}\":{value},"), "", 1);
        assert_ne!(missing, encoded, "field was not removed: {field}");
        let error = serde_json::from_str::<StoreSnapshot>(&missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field: {field}")),
            "{error}"
        );
    }
    for code in [
        AccountCode::Unknown,
        AccountCode::Empty,
        AccountCode::Runtime("0x00".into()),
        AccountCode::Delegation(format!("0x{}", "22".repeat(20))),
    ] {
        let mut store = snapshot.clone();
        store.code_default = Some(code);
        roundtrip(store);
    }
}
