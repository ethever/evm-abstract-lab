//! Exact word recipes are checked against primitive semantics and source data.
//! The retained witness is never replaced by a pretty expression arena record.
use evm_decompile_model::{
    Block, Effects, EntityId, ExitKind, Ir, LiftedBlock, LiftedExpression,
    LiftedExpressionKind as K, LiftedProgram, LiftedRenderError, LiftedStatement, LiftedType,
    Operation, SourceSpan, Terminator, U256, Value, ValueFact, ValueKind, WordExpr, WordRewrite,
    WordSimplifyError, simplify_word,
};
use std::collections::BTreeMap;

fn id(value: u64) -> EntityId {
    EntityId(value)
}
const BLOCK: EntityId = EntityId(100);
fn expression(n: u64, kind: K) -> LiftedExpression {
    LiftedExpression {
        id: id(n),
        block: BLOCK,
        ty: kind.ty(),
        kind,
    }
}
fn operation(n: u64, opcode: u8, inputs: &[u64], outputs: &[u64]) -> Operation {
    Operation {
        id: id(n),
        source: Some(SourceSpan {
            pc: n as usize,
            end_pc: n as usize + 1,
        }),
        opcode,
        name: match opcode {
            0x54 => "SLOAD",
            0x16 => "AND",
            0x02 => "MUL",
            0x60 => "PUSH1",
            0x00 => "STOP",
            _ => "WORD",
        }
        .into(),
        inputs: inputs.iter().copied().map(id).collect(),
        outputs: outputs.iter().copied().map(id).collect(),
        effects: Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    }
}
fn value(n: u64, kind: ValueKind) -> Value {
    Value {
        id: id(n),
        kind,
        fact: ValueFact::Unknown,
    }
}

fn fixture(inner: U256, outer: U256) -> (Ir, LiftedProgram) {
    let mut read = operation(3, 0x54, &[2], &[7]);
    read.effects.storage_read = true;
    read.effect_input = Some(id(1001));
    read.effect_output = Some(id(1002));
    let mut stop = operation(18, 0x00, &[], &[]);
    stop.effects.terminate = true;
    let ir = Ir {
        entry: Some(BLOCK),
        bytecode_len: 100,
        blocks: vec![Block {
            id: BLOCK,
            source: Some(SourceSpan { pc: 0, end_pc: 100 }),
            parameters: vec![],
            memory_parameter: id(1000),
            effect_parameter: id(1001),
            operations: vec![
                operation(1, 0x60, &[], &[2]),
                read,
                operation(8, 0x60, &[], &[9]),
                operation(10, 0x16, &[9, 7], &[11]),
                operation(12, 0x60, &[], &[13]),
                operation(15, 0x16, &[13, 11], &[16]),
                stop,
            ],
            terminator: Terminator::Exit {
                kind: ExitKind::Stop,
                values: vec![],
            },
        }],
        values: vec![
            value(2, ValueKind::Constant(U256::from(10))),
            value(
                7,
                ValueKind::Operation {
                    operation: id(3),
                    index: 0,
                },
            ),
            value(9, ValueKind::Constant(inner)),
            value(
                11,
                ValueKind::Operation {
                    operation: id(10),
                    index: 0,
                },
            ),
            value(13, ValueKind::Constant(outer)),
            value(
                16,
                ValueKind::Operation {
                    operation: id(15),
                    index: 0,
                },
            ),
            value(1000, ValueKind::MemoryParameter { block: BLOCK }),
            value(1001, ValueKind::EffectParameter { block: BLOCK }),
            value(
                1002,
                ValueKind::Effect {
                    operation: Some(id(3)),
                },
            ),
        ],
        ..Ir::default()
    };
    let program = LiftedProgram {
        expressions: vec![
            expression(201, K::Word(U256::from(10))),
            expression(202, K::Value(id(7))),
            expression(203, K::Word(inner)),
            expression(
                204,
                K::WordOp {
                    opcode: 0x16,
                    arguments: vec![id(203), id(202)],
                },
            ),
            expression(205, K::Word(outer)),
        ],
        blocks: vec![LiftedBlock {
            id: id(300),
            block: BLOCK,
            parameters: vec![],
            statements: vec![
                LiftedStatement {
                    id: id(301),
                    operation: id(3),
                    inputs: vec![id(201)],
                    outputs: vec![id(7)],
                },
                LiftedStatement {
                    id: id(302),
                    operation: id(15),
                    inputs: vec![id(205), id(204)],
                    outputs: vec![id(16)],
                },
            ],
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        ..LiftedProgram::default()
    };
    program.validate().unwrap();
    (ir, program)
}
fn statement(program: &LiftedProgram) -> &LiftedStatement {
    &program.blocks[0].statements[1]
}

#[test]
fn registered_nested_mask_view_binds_the_original_output_without_rewriting_witness() {
    for (inner, outer, expected) in [
        (0xff, 0xff, "v16 = (v7 & 0xff);"),
        (0xf0, 0x3f, "v16 = (v7 & 0x30);"),
        (0xf0, 0x0f, "v16 = 0x0;"),
    ] {
        let (ir, program) = fixture(U256::from(inner), U256::from(outer));
        let original = (ir.clone(), program.clone());
        let renderer = program.render_index(&ir);
        assert!(
            renderer
                .statement_word_rhs(statement(&program), |_| true)
                .unwrap()
                .is_some()
        );
        for dependencies in [false, true] {
            assert_eq!(
                renderer
                    .render_statement_with_dependencies(statement(&program), dependencies, |_| true)
                    .unwrap(),
                expected
            );
        }
        assert_eq!(statement(&program).inputs, vec![id(205), id(204)]);
        assert!(
            renderer
                .render_invocation(0x16, "and", &statement(&program).inputs, |_| true)
                .unwrap()
                .starts_with("evm.and(")
        );
        assert_eq!((ir, program), original);
    }
}

#[test]
fn unsupported_or_unproved_statements_keep_the_primitive_invocation() {
    for mutation in 0..14 {
        let (mut ir, mut program) = fixture(U256::from(255), U256::from(255));
        match mutation {
            0 => ir.blocks[0].terminator = Terminator::Pending,
            1 => {
                ir.blocks[0].operations.pop();
                ir.blocks[0].terminator = Terminator::Trap {
                    reason: "stack underflow".into(),
                };
            }
            2 => ir.blocks[0].operations[5].effects.world_read = true,
            3 => ir.blocks[0].operations[5].memory_input = Some(id(1000)),
            4 => ir.blocks[0].operations[5].outputs.push(id(17)),
            5 => ir.blocks[0].operations[5].inputs.pop().map(|_| ()).unwrap(),
            6 => {
                program.expressions[1].block = id(999);
            }
            7 => {
                program.expressions[2].kind = K::Word(U256::from(7));
            }
            8 => {
                program.blocks[0].statements.remove(0);
            }
            9 => {
                program.expressions[1].kind = K::Value(id(16));
            }
            10 => program.expressions[2].ty = LiftedType::Predicate,
            11 => program.expressions[4].kind = K::Word(U256::ZERO),
            12 => {
                program.expressions[3].kind = K::WordOp {
                    opcode: 0x16,
                    arguments: vec![id(204), id(203)],
                };
            }
            13 => {
                ir.values.iter_mut().find(|v| v.id == id(7)).unwrap().kind =
                    ValueKind::Constant(U256::ZERO);
                program.expressions[1].kind = K::Word(U256::ZERO);
            }
            _ => unreachable!(),
        }
        let renderer = program.render_index(&ir);
        assert_eq!(
            renderer
                .statement_word_rhs(program.blocks[0].statements.last().unwrap(), |_| true)
                .unwrap(),
            None,
            "mutation {mutation}"
        );
        let rendered =
            renderer.render_statement(program.blocks[0].statements.last().unwrap(), |_| true);
        if mutation == 12 {
            assert_eq!(rendered, Err(LiftedRenderError::Limit));
            continue;
        }
        let text = rendered.unwrap();
        assert!(
            text.starts_with("v16 = evm.and("),
            "mutation {mutation}: {text}"
        );
    }
    let (ir, program) = fixture(U256::from(255), U256::from(255));
    let mut unregistered = statement(&program).clone();
    unregistered.id = id(9999);
    assert_eq!(
        program
            .render_index(&ir)
            .statement_word_rhs(&unregistered, |_| true)
            .unwrap(),
        None
    );
}

fn normalized_source_masks() -> (Ir, LiftedProgram) {
    let (mut ir, mut program) = fixture(U256::from(255), U256::from(63));
    ir.blocks[0].operations[5] = operation(14, 0x16, &[13, 11], &[15]);
    ir.blocks[0]
        .operations
        .insert(6, operation(21, 0x60, &[], &[22]));
    ir.blocks[0]
        .operations
        .insert(7, operation(23, 0x16, &[22, 15], &[16]));
    ir.values.push(value(
        15,
        ValueKind::Operation {
            operation: id(14),
            index: 0,
        },
    ));
    ir.values
        .push(value(22, ValueKind::Constant(U256::from(15))));
    ir.values.iter_mut().find(|v| v.id == id(16)).unwrap().kind = ValueKind::Operation {
        operation: id(23),
        index: 0,
    };
    program.expressions[2].kind = K::Word(U256::from(63));
    program.expressions[3].kind = K::WordOp {
        opcode: 0x16,
        arguments: vec![id(202), id(203)],
    };
    program.expressions[4].kind = K::Word(U256::from(15));
    program.blocks[0].statements[1].operation = id(23);
    program.validate().unwrap();
    (ir, program)
}

#[test]
fn normalized_raw_mask_correspondence_rejects_both_forward_literal_and_raw_value() {
    let (ir, program) = normalized_source_masks();
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer
            .render_statement(statement(&program), |_| true)
            .unwrap(),
        "v16 = (v7 & 0xf);"
    );

    let mut forward = program.clone();
    forward.expressions.swap(2, 3); // mask literal is now after its AND parent.
    assert!(forward.validate().is_err());
    assert_eq!(
        forward
            .render_index(&ir)
            .statement_word_rhs(statement(&forward), |_| true)
            .unwrap(),
        None
    );

    let (mut ir, mut program) = normalized_source_masks();
    // Source inner AND at position 3 illegally reads a literal produced at 4.
    // It is available to the later AND, which must not hide that earlier fault.
    ir.blocks[0]
        .operations
        .insert(4, operation(24, 0x60, &[], &[25]));
    ir.values
        .push(value(25, ValueKind::Constant(U256::from(127))));
    ir.blocks[0].operations[3].inputs[1] = id(25);
    program.expressions[1].kind = K::Word(U256::from(127));
    assert_eq!(
        program
            .render_index(&ir)
            .statement_word_rhs(statement(&program), |_| true)
            .unwrap(),
        None
    );
}

#[test]
fn missing_leaf_and_wrong_registered_statement_cannot_obtain_a_recipe() {
    let (ir, mut program) = fixture(U256::from(255), U256::from(255));
    program.expressions[3].kind = K::WordOp {
        opcode: 0x16,
        arguments: vec![id(99999), id(203)],
    };
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer
            .statement_word_rhs(statement(&program), |_| true)
            .unwrap(),
        None
    );
    assert!(
        renderer
            .render_statement(statement(&program), |_| true)
            .is_err()
    );
    let (ir, program) = fixture(U256::from(255), U256::from(255));
    let mut copied = statement(&program).clone();
    copied.inputs.swap(0, 1);
    assert_eq!(
        program
            .render_index(&ir)
            .statement_word_rhs(&copied, |_| true)
            .unwrap(),
        None
    );
}

#[test]
fn predicate_cast_correspondence_is_conservative_even_for_an_identity_root() {
    let (mut ir, mut program) = fixture(U256::from(255), U256::ZERO);
    ir.blocks[0].operations[3].opcode = 0x10;
    ir.blocks[0].operations[3].name = "LT".into();
    ir.blocks[0].operations[5].opcode = 0x01;
    ir.blocks[0].operations[5].name = "ADD".into();
    program.expressions.insert(
        3,
        expression(
            206,
            K::Compare {
                relation: evm_decompile_model::LiftedRelation::Ult,
                left: id(203),
                right: id(202),
            },
        ),
    );
    program.expressions[4].kind = K::WordFromPredicate(id(206));
    program.validate().unwrap();
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer
            .statement_word_rhs(statement(&program), |_| true)
            .unwrap(),
        None
    );
    assert!(
        renderer
            .render_statement(statement(&program), |_| true)
            .unwrap()
            .starts_with("v16 = evm.add(")
    );
}

#[test]
fn a_foreign_copy_alias_is_not_inlined_from_its_producer() {
    let (mut ir, mut program) = fixture(U256::from(255), U256::from(255));
    let prefix = ir.blocks[0].operations.drain(..2).collect();
    ir.blocks[0].parameters = vec![id(21)];
    ir.blocks[0].operations[1].inputs[1] = id(21);
    ir.blocks[0].memory_parameter = id(2000);
    ir.blocks[0].effect_parameter = id(2001);
    ir.blocks.insert(
        0,
        Block {
            id: id(90),
            source: Some(SourceSpan { pc: 0, end_pc: 5 }),
            parameters: vec![],
            memory_parameter: id(1000),
            effect_parameter: id(1001),
            operations: prefix,
            terminator: Terminator::Fallthrough { edge: id(50) },
        },
    );
    ir.entry = Some(id(90));
    ir.edges.push(evm_decompile_model::Edge {
        id: id(50),
        from: id(90),
        to: BLOCK,
        kind: evm_decompile_model::EdgeKind::Fallthrough,
        arguments: vec![id(7)],
        memory: id(1000),
        effect: id(1002),
    });
    ir.values.push(value(
        21,
        ValueKind::Parameter {
            block: BLOCK,
            index: 0,
        },
    ));
    ir.values
        .push(value(2000, ValueKind::MemoryParameter { block: BLOCK }));
    ir.values
        .push(value(2001, ValueKind::EffectParameter { block: BLOCK }));
    for val in &mut ir.values {
        match &mut val.kind {
            ValueKind::MemoryParameter { block } | ValueKind::EffectParameter { block }
                if val.id == id(1000) || val.id == id(1001) =>
            {
                *block = id(90)
            }
            _ => {}
        }
    }
    let read = program.blocks[0].statements.remove(0);
    program.blocks.insert(
        0,
        LiftedBlock {
            id: id(290),
            block: id(90),
            parameters: vec![],
            statements: vec![read],
            condition: None,
            target: None,
            exit_values: vec![],
        },
    );
    program.expressions[0].block = id(90);
    // The foreign v7 is available through an identity copy at entry. This model
    // view deliberately does not rebuild the native cross-block alias proof.
    program.validate().unwrap();
    let statement = &program.blocks[1].statements[0];
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer.statement_word_rhs(statement, |_| true).unwrap(),
        None
    );
    assert!(
        renderer
            .render_statement(statement, |_| true)
            .unwrap()
            .starts_with("v16 = evm.and(")
    );
}

#[test]
fn every_rule_and_render_payment_observes_cancellation_without_a_partial_result() {
    let (ir, program) = fixture(U256::from(0xf0), U256::from(0x3f));
    let renderer = program.render_index(&ir);
    let mut payments = Vec::new();
    assert!(
        renderer
            .statement_word_rhs(statement(&program), |amount| {
                payments.push(amount);
                true
            })
            .unwrap()
            .is_some()
    );
    for fail in 0..payments.len() {
        let mut calls = 0;
        assert_eq!(
            renderer.statement_word_rhs(statement(&program), |_| {
                let admit = calls < fail;
                calls += 1;
                admit
            }),
            Err(LiftedRenderError::Limit),
            "rule payment {fail}"
        );
    }
    payments.clear();
    renderer
        .render_statement(statement(&program), |amount| {
            payments.push(amount);
            true
        })
        .unwrap();
    for fail in 0..payments.len() {
        let mut calls = 0;
        assert_eq!(
            renderer.render_statement(statement(&program), |_| {
                let admit = calls < fail;
                calls += 1;
                admit
            }),
            Err(LiftedRenderError::Limit),
            "render payment {fail}"
        );
    }
}

#[test]
fn unchanged_root_does_not_traverse_an_exponentially_shared_input_dag() {
    let (mut ir, mut program) = fixture(U256::from(255), U256::from(3));
    ir.blocks[0].operations[5].opcode = 0x02;
    let mut child = id(202);
    for n in 400..440 {
        program.expressions.push(expression(
            n,
            K::WordOp {
                opcode: 0x02,
                arguments: vec![child, child],
            },
        ));
        child = id(n);
    }
    program.blocks[0].statements[1].inputs[1] = child;
    program.validate().unwrap();
    let renderer = program.render_index(&ir);
    let mut paid = 0;
    assert_eq!(
        renderer
            .statement_word_rhs(statement(&program), |amount| {
                paid += amount;
                paid < 12
            })
            .unwrap(),
        None
    );
    assert!(paid < 12, "noncandidate performed {paid} work");
}

#[test]
fn malformed_reference_vectors_are_quoted_before_source_index_construction() {
    let (mut ir, mut program) = fixture(U256::from(255), U256::from(255));
    ir.blocks[0].operations[5].outputs = vec![id(16); 1500];
    program.blocks[0].parameters = vec![id(7000); 1200];
    program.blocks[0].statements[1].outputs = vec![id(16); 1300];
    let mut calls = 0;
    let renderer = program.render_index_with_budget(&ir, |amount| {
        calls += 1;
        assert!(amount >= 6 * (1500 + 1200 + 1300));
        false
    });
    assert_eq!(calls, 1);
    assert!(renderer.block(BLOCK).is_none());
    assert_eq!(
        renderer
            .statement_word_rhs(statement(&program), |_| true)
            .unwrap(),
        None
    );
}

fn recipe_value(recipe: WordRewrite, words: &BTreeMap<EntityId, U256>) -> Option<U256> {
    match recipe {
        WordRewrite::Unchanged => None,
        WordRewrite::Existing(id) => Some(words[&id]),
        WordRewrite::Literal(word) => Some(word),
        WordRewrite::Masked { value, mask } => Some(words[&value] & mask),
    }
}

#[test]
fn shared_word_identities_and_masks_match_independent_primitive_evaluation() {
    let boundaries = [
        U256::ZERO,
        U256::from(1),
        U256::from(2),
        U256::from(255),
        U256::from(1) << 255,
        U256::MAX,
    ];
    for opcode in [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x16, 0x17, 0x18, 0x1b, 0x1c, 0x1d,
    ] {
        for constant in [U256::ZERO, U256::from(1), U256::MAX] {
            for reversed in [false, true] {
                let args = if reversed {
                    [id(2), id(1)]
                } else {
                    [id(1), id(2)]
                };
                let recipe = simplify_word(
                    opcode,
                    &args,
                    |i| {
                        Some(if i == id(1) {
                            WordExpr::Opaque
                        } else {
                            WordExpr::Literal(constant)
                        })
                    },
                    |_| true,
                )
                .unwrap();
                for word in boundaries {
                    let values = BTreeMap::from([(id(1), word), (id(2), constant)]);
                    let original =
                        evm_bytecode::word::evaluate(opcode, &args.map(|id| values[&id])).unwrap();
                    if let Some(rewritten) = recipe_value(recipe, &values) {
                        assert_eq!(
                            rewritten, original,
                            "opcode={opcode:#x},constant={constant},word={word}"
                        );
                    }
                }
            }
        }
    }
    for (a, b) in [(0xff, 0xff), (0xff, 0x3f), (0xf0, 0x0f)] {
        for reversed in [false, true] {
            let args = if reversed {
                [id(4), id(3)]
            } else {
                [id(3), id(4)]
            };
            let recipe = simplify_word(
                0x16,
                &args,
                |i| {
                    Some(match i.0 {
                        1 => WordExpr::Opaque,
                        2 => WordExpr::Literal(U256::from(a)),
                        3 => WordExpr::And {
                            left: id(1),
                            right: id(2),
                        },
                        4 => WordExpr::Literal(U256::from(b)),
                        _ => unreachable!(),
                    })
                },
                |_| true,
            )
            .unwrap();
            for word in boundaries {
                let values = BTreeMap::from([
                    (id(1), word),
                    (id(2), U256::from(a)),
                    (id(3), word & U256::from(a)),
                    (id(4), U256::from(b)),
                ]);
                assert_eq!(
                    recipe_value(recipe, &values),
                    Some((word & U256::from(a)) & U256::from(b))
                );
            }
        }
    }
}

#[test]
fn constant_evaluation_and_lookup_refusals_are_typed_and_paid_before_work() {
    for opcode in [0x01, 0x08, 0x0a, 0x0b, 0x19, 0x1d, 0x1e] {
        let arity = evm_bytecode::word::arity(opcode).unwrap();
        let args: Vec<_> = (1..=arity as u64).map(id).collect();
        let mut payments = Vec::new();
        let recipe = simplify_word(
            opcode,
            &args,
            |i| Some(WordExpr::Literal(U256::from(i.0))),
            |amount| {
                payments.push(amount);
                true
            },
        )
        .unwrap();
        assert_eq!(
            recipe,
            WordRewrite::Literal(
                evm_bytecode::word::evaluate(
                    opcode,
                    &(1..=arity).map(U256::from).collect::<Vec<_>>()
                )
                .unwrap()
            )
        );
        for fail in 0..payments.len() {
            let mut calls = 0;
            assert_eq!(
                simplify_word(
                    opcode,
                    &args,
                    |i| Some(WordExpr::Literal(U256::from(i.0))),
                    |_| {
                        let ok = calls < fail;
                        calls += 1;
                        ok
                    }
                ),
                Err(WordSimplifyError::Limit)
            );
        }
    }
    for opcode in 0x10..=0x15 {
        assert_eq!(
            simplify_word(
                opcode,
                &[id(1), id(2)],
                |_| Some(WordExpr::Literal(U256::ZERO)),
                |_| true
            ),
            Err(WordSimplifyError::Unsupported)
        );
    }
    assert_eq!(
        simplify_word(0x16, &[id(1), id(2)], |_| None, |_| true),
        Err(WordSimplifyError::Missing(id(1)))
    );
    let mut lookups = 0;
    assert_eq!(
        simplify_word(
            0x16,
            &[id(1), id(2)],
            |_| {
                lookups += 1;
                None
            },
            |_| false
        ),
        Err(WordSimplifyError::Limit)
    );
    assert_eq!(lookups, 0);
}
