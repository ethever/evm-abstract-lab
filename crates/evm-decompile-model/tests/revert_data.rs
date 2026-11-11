//! Data identity, wire safety and decoding are independent of numeric recovery.
use evm_decompile_model::*;
use serde_json::json;
fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn data(content: LiftedRevertDataKind) -> LiftedRevertData {
    LiftedRevertData {
        id: id(900),
        execution_domain: ExecutionDomain::Generic,
        execution_fork: None,
        code_hash: None,
        block: id(100),
        operation: id(200),
        memory: id(301),
        offset: id(400),
        size: id(401),
        content,
        writes: vec![id(201)],
    }
}
fn parts(selector: u32, word: LiftedDataWord) -> LiftedRevertDataKind {
    LiftedRevertDataKind::Segments(vec![
        LiftedByteSegment::Word {
            value: LiftedDataWord::Constant(U256::from(selector) << 224),
            offset: 0,
            size: 4,
        },
        LiftedByteSegment::Word {
            value: word,
            offset: 0,
            size: 32,
        },
    ])
}
#[test]
fn derived_formats_keep_symbolic_words_and_do_not_claim_fragmented_arguments() {
    for code in [0_u64, 0x41, 0xff] {
        let proof = data(parts(
            0x4e487b71,
            LiftedDataWord::Constant(U256::from(code)),
        ));
        let f = proof.decode_format(|_| true).unwrap();
        assert!(matches!(f, LiftedRevertFormat::Panic(_)));
        assert_eq!(f.panic_explanation().is_some(), code != 0xff);
    }
    let mut proof = data(parts(0xdeadbeef, LiftedDataWord::SourceValue(id(600))));
    assert_eq!(
        proof.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::SelectorWords {
            selector: [0xde, 0xad, 0xbe, 0xef],
            words: vec![LiftedDataWord::SourceValue(id(600))]
        }
    );
    let LiftedRevertDataKind::Segments(parts) = &mut proof.content else {
        unreachable!()
    };
    parts[1] = LiftedByteSegment::Word {
        value: LiftedDataWord::SourceValue(id(600)),
        offset: 1,
        size: 31,
    };
    assert_eq!(
        proof.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::Raw
    );
    let untyped = data(LiftedRevertDataKind::Segments(vec![
        LiftedByteSegment::Bytes(vec![0xab; 36]),
    ]));
    assert_eq!(
        untyped.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::Raw
    );
    assert!(matches!(untyped.content, LiftedRevertDataKind::Segments(_)));
}
fn error_bytes(message: &[u8]) -> Vec<u8> {
    let mut bytes = vec![8, 0xc3, 0x79, 0xa0];
    bytes.extend(U256::from(32).to_be_bytes::<32>());
    bytes.extend(U256::from(message.len()).to_be_bytes::<32>());
    bytes.extend(message);
    bytes.resize(68 + message.len().div_ceil(32) * 32, 0);
    bytes
}
#[test]
fn invalid_or_huge_string_headers_remain_recovered_data_without_panicking() {
    let good = data(LiftedRevertDataKind::Segments(vec![
        LiftedByteSegment::Bytes(error_bytes(b"hello")),
    ]));
    assert_eq!(
        good.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::ErrorString("hello".into())
    );
    let mut cases = vec![error_bytes(&[0xff]), error_bytes(b"x")];
    *cases[1].last_mut().unwrap() = 1;
    for length in [U256::from(usize::MAX), U256::from(usize::MAX - 31)] {
        let mut bytes = error_bytes(b"x");
        bytes[36..68].copy_from_slice(&length.to_be_bytes::<32>());
        cases.push(bytes);
    }
    for bytes in cases {
        let proof = data(LiftedRevertDataKind::Segments(vec![
            LiftedByteSegment::Bytes(bytes),
        ]));
        assert_eq!(
            proof.decode_format(|_| true).unwrap(),
            LiftedRevertFormat::Raw
        );
        assert!(matches!(proof.content, LiftedRevertDataKind::Segments(_)));
    }
}
#[test]
fn raw_empty_and_forwarded_are_distinct_and_refusal_returns_no_format() {
    let empty = data(LiftedRevertDataKind::Segments(vec![]));
    assert_eq!(
        empty.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::Empty
    );
    assert_eq!(
        data(LiftedRevertDataKind::Raw)
            .decode_format(|_| true)
            .unwrap(),
        LiftedRevertFormat::Raw
    );
    let proof = data(LiftedRevertDataKind::Segments(vec![
        LiftedByteSegment::Slice {
            source: LiftedByteSource::ReturnData(id(80)),
            offset: LiftedDataWord::Constant(U256::ZERO),
            size: LiftedDataWord::SourceValue(id(81)),
        },
    ]));
    assert!(matches!(
        proof.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::Forwarded { .. }
    ));
    for allowance in [0, 1] {
        let mut calls = 0;
        assert_eq!(
            proof.decode_format(|_| {
                calls += 1;
                calls <= allowance
            }),
            if allowance == 0 {
                Err(LiftedRenderError::Limit)
            } else {
                Ok(LiftedRevertFormat::Forwarded {
                    source: LiftedByteSource::ReturnData(id(80)),
                    offset: LiftedDataWord::Constant(U256::ZERO),
                    size: LiftedDataWord::SourceValue(id(81)),
                })
            }
        );
    }
}
#[test]
fn strict_weighted_wire_and_schema_gate_keep_legacy_artifacts_unchanged() {
    let proof = data(parts(0xdeadbeef, LiftedDataWord::SourceValue(id(600))));
    let bytes = serde_json::to_vec(&proof).unwrap();
    assert_eq!(
        serde_json::from_slice::<LiftedRevertData>(&bytes).unwrap(),
        proof
    );
    for mutation in ["unknown", "zero", "fragment", "no_content"] {
        let mut wire = serde_json::to_value(&proof).unwrap();
        match mutation {
            "unknown" => wire["extra"] = json!(1),
            "zero" => wire["content"]["segments"][1]["word"]["value"] = json!({"source_value":"0"}),
            "fragment" => wire["content"]["segments"][1]["word"]["size"] = json!(33),
            _ => {
                wire.as_object_mut().unwrap().remove("content");
            }
        }
        assert!(
            serde_json::from_value::<LiftedRevertData>(wire).is_err(),
            "{mutation}"
        );
    }
    let legacy = include_str!("fixtures/partial-artifact.json").trim_end();
    let mut artifact: Artifact = serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_string(&artifact).unwrap(), legacy);
    artifact.schema_version = 10;
    assert!(artifact.admits_schema());
    artifact.stages.last_mut().unwrap().ir.lifted = Some(LiftedProgram {
        revert_data: vec![proof],
        ..LiftedProgram::default()
    });
    assert!(!artifact.admits_schema());
    artifact.schema_version = SCHEMA_VERSION;
    assert_eq!(SCHEMA_VERSION, 11);
    assert!(artifact.admits_schema());
}
#[test]
fn new_data_weight_is_in_the_shared_arena_budget_and_bad_prefix_stops_early() {
    let proof = data(parts(
        0x4e487b71,
        LiftedDataWord::Constant(U256::from(0x41)),
    ));
    let program = LiftedProgram {
        revert_data: vec![proof.clone()],
        ..LiftedProgram::default()
    };
    assert_eq!(program.record_count(), proof.record_count());
    let part = format!(
        "{{\"bytes\":[{}]}}",
        vec!["0"; MAX_REVERT_DATA_BYTES].join(",")
    );
    let count = MAX_LIFTED_RECORDS / (MAX_REVERT_DATA_BYTES + 1) + 1;
    let prefix = format!(
        "{{\"id\":\"900\",\"block\":\"100\",\"operation\":\"200\",\"memory\":\"301\",\"offset\":\"400\",\"size\":\"401\",\"content\":{{\"segments\":[{},{{MALFORMED",
        vec![part; count].join(",")
    );
    assert!(
        serde_json::from_str::<LiftedRevertData>(&prefix)
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
}

// Lightweight source-boundary fixture, not a numeric byte-recovery proof.
fn source() -> Ir {
    let operation = |n: u64,
                     opcode: u8,
                     inputs: Vec<EntityId>,
                     memory_input: u64,
                     memory_output: Option<u64>| Operation {
        id: id(n),
        source: None,
        opcode,
        name: if opcode == 0x52 {
            "MSTORE".into()
        } else {
            "REVERT".into()
        },
        inputs,
        outputs: vec![],
        effects: Effects {
            memory_write: opcode == 0x52,
            memory_read: opcode == 0xfd,
            terminate: opcode == 0xfd,
            ..Effects::default()
        },
        memory_input: Some(id(memory_input)),
        memory_output: memory_output.map(id),
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let mut proof = data(LiftedRevertDataKind::Segments(vec![
        LiftedByteSegment::Word {
            value: LiftedDataWord::SourceValue(id(600)),
            offset: 0,
            size: 32,
        },
    ]));
    proof.writes = vec![id(201)];
    Ir {
        entry: Some(id(100)),
        blocks: vec![Block {
            id: id(100),
            source: None,
            parameters: vec![id(600)],
            memory_parameter: id(300),
            effect_parameter: id(350),
            operations: vec![
                operation(201, 0x52, vec![id(400), id(600)], 300, Some(301)),
                operation(200, 0xfd, vec![id(400), id(401)], 301, None),
            ],
            terminator: Terminator::Exit {
                kind: ExitKind::Revert,
                values: vec![id(400), id(401)],
            },
        }],
        values: vec![
            Value {
                id: id(400),
                kind: ValueKind::Constant(U256::ZERO),
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(401),
                kind: ValueKind::Constant(U256::from(32)),
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(600),
                kind: ValueKind::Parameter {
                    block: id(100),
                    index: 0,
                },
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(699),
                kind: ValueKind::Constant(U256::from(99)),
                fact: ValueFact::Unknown,
            },
        ],
        structured: Some(StructuredProgram {
            root: StructuredNode {
                id: id(5000),
                kind: StructuredKind::Sequence(vec![
                    StructuredNode {
                        id: id(5001),
                        kind: StructuredKind::Block { block: id(100) },
                    },
                    StructuredNode {
                        id: id(5002),
                        kind: StructuredKind::Exit { block: id(100) },
                    },
                ]),
            },
        }),
        lifted: Some(LiftedProgram {
            expressions: vec![
                LiftedExpression {
                    id: id(1700),
                    block: id(100),
                    ty: LiftedType::Word256,
                    kind: LiftedExpressionKind::Word(U256::ZERO),
                },
                LiftedExpression {
                    id: id(1701),
                    block: id(100),
                    ty: LiftedType::Word256,
                    kind: LiftedExpressionKind::Value(id(600)),
                },
                LiftedExpression {
                    id: id(1702),
                    block: id(100),
                    ty: LiftedType::Word256,
                    kind: LiftedExpressionKind::Word(U256::from(32)),
                },
            ],
            blocks: vec![LiftedBlock {
                id: id(1500),
                block: id(100),
                parameters: vec![id(600)],
                statements: vec![
                    LiftedStatement {
                        id: id(1501),
                        operation: id(201),
                        inputs: vec![id(1700), id(1701)],
                        outputs: vec![],
                    },
                    LiftedStatement {
                        id: id(1502),
                        operation: id(200),
                        inputs: vec![id(1700), id(1702)],
                        outputs: vec![],
                    },
                ],
                condition: None,
                target: None,
                exit_values: vec![id(1700), id(1702)],
            }],
            revert_data: vec![proof],
            ..LiftedProgram::default()
        }),
        ..Ir::default()
    }
}

#[test]
fn source_word_identity_requires_the_actual_writer_and_its_scoped_high_operand() {
    let original = source();
    let p = original.lifted.as_ref().unwrap();
    p.validate_guard_sources(&original).unwrap();
    let renderer = p.render_index(&original);
    assert!(
        renderer
            .render_revert_data(id(900), |_| true)
            .unwrap()
            .contains("word_bytes(v600")
    );
    for mutation in [
        "foreign_value",
        "missing_high_operand",
        "wrong_high_operand",
        "foreign_scope",
        "memory",
    ] {
        let mut ir = original.clone();
        let p = ir.lifted.as_mut().unwrap();
        match mutation {
            "foreign_value" => {
                p.revert_data[0].content =
                    LiftedRevertDataKind::Segments(vec![LiftedByteSegment::Word {
                        value: LiftedDataWord::SourceValue(id(699)),
                        offset: 0,
                        size: 32,
                    }])
            }
            "missing_high_operand" => {
                p.blocks[0].statements[0].inputs.pop();
            }
            "wrong_high_operand" => p.blocks[0].statements[0].inputs[1] = id(1702),
            "foreign_scope" => p.expressions[1].block = id(999),
            _ => p.revert_data[0].memory = id(300),
        }
        let p = ir.lifted.as_ref().unwrap();
        assert!(p.validate_guard_sources(&ir).is_err(), "{mutation}");
        let renderer = p.render_index(&ir);
        assert!(renderer.revert_data(id(900)).is_none(), "{mutation}");
    }
    for allowance in [0, 1, 4] {
        let mut calls = 0;
        assert_eq!(
            renderer.render_revert_data(id(900), |_| {
                calls += 1;
                calls <= allowance
            }),
            Err(LiftedRenderError::Limit)
        );
    }
}

#[test]
fn recovered_unrecognized_data_stays_inspectable_but_uses_the_original_terminal() {
    let ir = source();
    let before = serde_json::to_vec(&ir).unwrap();
    let program = ir.lifted.as_ref().unwrap();
    let data = &program.revert_data[0];
    assert!(matches!(data.content, LiftedRevertDataKind::Segments(_)));
    assert_eq!(
        data.decode_format(|_| true).unwrap(),
        LiftedRevertFormat::Raw
    );
    let renderer = program.render_index(&ir);
    assert!(
        renderer
            .render_revert_data(data.id, |_| true)
            .unwrap()
            .contains("concat(word_bytes(v600, 0, 32))"),
        "format recognition must not discard the recovered source word"
    );
    assert_eq!(
        renderer.render_data_revert(data.block, |_| true).unwrap(),
        None
    );
    let terminal = &program.blocks[0].statements[1];
    assert_eq!(
        renderer
            .render_statement_with_dependencies(terminal, false, |_| true)
            .unwrap(),
        "evm.revert(offset = 0x0, size = 0x20);"
    );
    assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
}
