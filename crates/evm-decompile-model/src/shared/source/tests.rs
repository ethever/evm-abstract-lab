//! Admission tests supply certificates directly, independently of discovery.
use super::*;
use crate::{
    EdgeKind, Effects, LiftedBlock, LiftedExpression, LiftedExpressionKind as X, LiftedStatement,
    LiftedTransfer, Operation, SharedProcedure, SharedProcedureKind as P, StructuredProgram, U256,
    ValueFact,
};

fn id(n: u64) -> EntityId {
    EntityId(n)
}

fn value(n: u64, kind: V) -> Value {
    Value {
        id: id(n),
        kind,
        fact: ValueFact::Unknown,
    }
}

fn expression(n: u64, kind: X) -> LiftedExpression {
    LiftedExpression {
        id: id(n),
        block: id(10),
        ty: kind.ty(),
        kind,
    }
}

fn expression_source() -> Ir {
    Ir {
        entry: Some(id(10)),
        values: vec![
            value(
                1,
                V::Parameter {
                    block: id(10),
                    index: 0,
                },
            ),
            value(100, V::Memory { operation: None }),
            value(101, V::Effect { operation: None }),
        ],
        blocks: vec![Block {
            id: id(10),
            source: None,
            parameters: vec![id(1)],
            memory_parameter: id(100),
            effect_parameter: id(101),
            operations: vec![],
            terminator: T::Pending,
        }],
        lifted: Some(LiftedProgram {
            expressions: vec![
                expression(1000, X::Value(id(1))),
                expression(1001, X::Word(U256::from(5))),
                expression(1002, X::Word(U256::from(7))),
                expression(
                    1003,
                    X::WordOp {
                        opcode: 0x02,
                        arguments: vec![id(1000), id(1001)],
                    },
                ),
                expression(
                    1004,
                    X::WordOp {
                        opcode: 0x01,
                        arguments: vec![id(1003), id(1002)],
                    },
                ),
                expression(1100, X::Value(id(1))),
                expression(
                    1101,
                    X::WordOp {
                        opcode: 0x02,
                        arguments: vec![id(1100), id(1100)],
                    },
                ),
                expression(
                    1102,
                    X::WordOp {
                        opcode: 0x01,
                        arguments: vec![id(1101), id(1100)],
                    },
                ),
                expression(1200, X::Predicate(true)),
                expression(1201, X::Not(id(1200))),
            ],
            blocks: vec![LiftedBlock {
                id: id(500),
                block: id(10),
                parameters: vec![id(1)],
                statements: vec![],
                condition: None,
                target: None,
                exit_values: vec![],
            }],
            ..LiftedProgram::default()
        }),
        ..Ir::default()
    }
}

fn loop_node(base: u64) -> StructuredNode {
    StructuredNode {
        id: id(base),
        kind: K::While {
            header: Box::new(StructuredNode {
                id: id(base + 1),
                kind: K::Block { block: id(10) },
            }),
            condition: None,
            condition_nonzero: true,
            body: Box::new(StructuredNode {
                id: id(base + 2),
                kind: K::Sequence(vec![
                    StructuredNode {
                        id: id(base + 3),
                        kind: K::Transfer { edge: id(30) },
                    },
                    StructuredNode {
                        id: id(base + 4),
                        kind: K::Continue,
                    },
                ]),
            }),
            exit_edge: None,
        },
    }
}

fn loop_source() -> Ir {
    Ir {
        entry: Some(id(10)),
        values: vec![
            value(
                1,
                V::Parameter {
                    block: id(10),
                    index: 0,
                },
            ),
            value(
                2,
                V::Operation {
                    operation: id(200),
                    index: 0,
                },
            ),
            value(3, V::Constant(U256::from(1))),
            value(100, V::Memory { operation: None }),
            value(101, V::Effect { operation: None }),
        ],
        blocks: vec![Block {
            id: id(10),
            source: None,
            parameters: vec![id(1)],
            memory_parameter: id(100),
            effect_parameter: id(101),
            operations: vec![Operation {
                id: id(200),
                source: None,
                opcode: 0x01,
                name: "ADD".into(),
                inputs: vec![id(1), id(3)],
                outputs: vec![id(2)],
                effects: Effects::default(),
                memory_input: None,
                memory_output: None,
                replacement: None,
                effect_input: None,
                effect_output: None,
            }],
            terminator: T::Fallthrough { edge: id(30) },
        }],
        edges: vec![Edge {
            id: id(30),
            from: id(10),
            to: id(10),
            kind: EdgeKind::Jump,
            arguments: vec![id(2)],
            memory: id(100),
            effect: id(101),
        }],
        structured: Some(StructuredProgram {
            root: StructuredNode {
                id: id(50),
                kind: K::Sequence(vec![loop_node(20), loop_node(40)]),
            },
        }),
        lifted: Some(LiftedProgram {
            expressions: vec![
                expression(1000, X::Value(id(1))),
                expression(1001, X::Word(U256::from(1))),
                expression(1002, X::Value(id(2))),
            ],
            blocks: vec![LiftedBlock {
                id: id(500),
                block: id(10),
                parameters: vec![id(1)],
                statements: vec![LiftedStatement {
                    id: id(300),
                    operation: id(200),
                    inputs: vec![id(1000), id(1001)],
                    outputs: vec![id(2)],
                }],
                condition: None,
                target: None,
                exit_values: vec![],
            }],
            transfers: vec![LiftedTransfer {
                id: id(501),
                edge: id(30),
                arguments: vec![id(1002)],
            }],
            ..LiftedProgram::default()
        }),
        ..Ir::default()
    }
}

#[test]
fn arithmetic_templates_parameterize_constants_and_allow_repeated_actuals() {
    let ir = expression_source();
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let canonical = index.arithmetic_region(id(1004), &mut |_| true).unwrap();
    let actual = index.arithmetic_region(id(1102), &mut |_| true).unwrap();
    assert_eq!(
        canonical.interface.inputs,
        vec![id(1000), id(1001), id(1002)]
    );
    assert_eq!(actual.interface.inputs, vec![id(1100)]);
    let bindings = index
        .correspondence(&canonical, &actual, &mut |_| true)
        .unwrap();
    index
        .verify_correspondence(&canonical, &actual, &bindings, &mut |_| true)
        .unwrap();
    assert!(
        index
            .source_region(P::Normal, id(1004), &mut |_| true)
            .is_err()
    );
    let predicate = index.arithmetic_region(id(1201), &mut |_| true).unwrap();
    assert!(
        index
            .correspondence(&canonical, &predicate, &mut |_| true)
            .is_err()
    );

    let mut supplied = ir.lifted.clone().unwrap();
    supplied.procedures.push(SharedProcedure {
        id: id(9000),
        kind: P::PureArithmetic,
        body: canonical.root,
        interface: canonical.interface,
    });
    supplied.invocations = vec![
        index
            .arithmetic_invocation(id(9001), id(9000), id(1004), vec![])
            .unwrap(),
        index
            .arithmetic_invocation(id(9002), id(9000), id(1102), bindings)
            .unwrap(),
    ];
    supplied.validate_shared_sources(&ir, |_| true).unwrap();
    supplied.invocations[1].owner = None;
    assert!(matches!(
        supplied.validate_shared_sources(&ir, |_| true),
        Err(Error::Site(_))
    ));
}

#[test]
fn pure_loop_procedures_need_no_termination_proof() {
    let ir = loop_source();
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let canonical = index
        .source_region(P::PureArithmetic, id(20), &mut |_| true)
        .unwrap();
    let actual = index
        .source_region(P::PureArithmetic, id(40), &mut |_| true)
        .unwrap();
    assert!(!canonical.nodes.is_empty());
    let bindings = index
        .correspondence(&canonical, &actual, &mut |_| true)
        .unwrap();
    let mut supplied = ir.lifted.clone().unwrap();
    supplied.procedures.push(SharedProcedure {
        id: id(9000),
        kind: P::PureArithmetic,
        body: canonical.root,
        interface: canonical.interface,
    });
    supplied.invocations = vec![
        index
            .invocation(id(9001), id(9000), id(20), vec![])
            .unwrap(),
        index
            .invocation(id(9002), id(9000), id(40), bindings)
            .unwrap(),
    ];
    supplied.validate_shared_sources(&ir, |_| true).unwrap();
}

#[test]
fn stateful_loop_contracts_cannot_be_relabelled_as_arithmetic_or_legacy() {
    let mut ir = loop_source();
    let op = &mut ir.blocks[0].operations[0];
    op.opcode = 0x54;
    op.effects.storage_read = true;
    op.effect_input = Some(id(101));
    op.effect_output = Some(id(102));
    ir.values.push(value(
        102,
        V::Effect {
            operation: Some(id(200)),
        },
    ));
    ir.edges[0].effect = id(102);
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let canonical = index
        .source_region(P::Normal, id(20), &mut |_| true)
        .unwrap();
    let actual = index
        .source_region(P::Normal, id(40), &mut |_| true)
        .unwrap();
    let bindings = index
        .correspondence(&canonical, &actual, &mut |_| true)
        .unwrap();
    let mut supplied = ir.lifted.clone().unwrap();
    supplied.procedures.push(SharedProcedure {
        id: id(9000),
        kind: P::Normal,
        body: canonical.root,
        interface: canonical.interface,
    });
    supplied.invocations = vec![
        index
            .invocation(id(9001), id(9000), id(20), vec![])
            .unwrap(),
        index
            .invocation(id(9002), id(9000), id(40), bindings)
            .unwrap(),
    ];
    supplied.validate_shared_sources(&ir, |_| true).unwrap();
    supplied.procedures[0].kind = P::PureArithmetic;
    assert!(matches!(
        supplied.validate_shared_sources(&ir, |_| true),
        Err(Error::Interface(_))
    ));
    supplied.procedures[0].kind = P::Legacy;
    assert!(matches!(
        supplied.validate_shared_sources(&ir, |_| true),
        Err(Error::Region(_))
    ));
}

#[test]
fn nested_expression_definitions_share_code_without_expansion_or_cycles() {
    let ir = expression_source();
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let mut supplied = ir.lifted.clone().unwrap();
    for (procedure, source) in [(9000, 1004), (9010, 1003)] {
        let region = index.arithmetic_region(id(source), &mut |_| true).unwrap();
        supplied.procedures.push(SharedProcedure {
            id: id(procedure),
            kind: P::PureArithmetic,
            body: id(source),
            interface: region.interface,
        });
        supplied.invocations.push(
            index
                .arithmetic_invocation(id(procedure + 1), id(procedure), id(source), vec![])
                .unwrap(),
        );
    }
    supplied.validate_shared_sources(&ir, |_| true).unwrap();
    // A supplied inner call cannot recursively reuse the containing definition
    // by pretending that a smaller source template has the same code domain.
    supplied.invocations[1].procedure = id(9000);
    assert!(supplied.validate_shared_sources(&ir, |_| true).is_err());
}

#[test]
fn a_leaf_only_definition_cannot_substitute_itself_into_its_argument() {
    let ir = expression_source();
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    assert!(matches!(
        index.arithmetic_region(id(1000), &mut |_| true),
        Err(Error::Region(_))
    ));
    let mut supplied = ir.lifted.clone().unwrap();
    supplied.procedures.push(SharedProcedure {
        id: id(9000),
        kind: P::PureArithmetic,
        body: id(1000),
        interface: SharedInterface {
            inputs: vec![id(1000)],
            ..SharedInterface::default()
        },
    });
    supplied.invocations.push(
        index
            .arithmetic_invocation(id(9001), id(9000), id(1000), vec![])
            .unwrap(),
    );
    supplied.validate().unwrap();
    assert!(matches!(
        supplied.validate_shared_sources(&ir, |_| true),
        Err(Error::Region(_))
    ));
}

#[test]
fn forbidden_raw_operations_cannot_hide_behind_an_effect_free_envelope() {
    for opcode in [
        0x00, 0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xfa, 0xfd, 0xfe, 0xff,
    ] {
        let mut ir = loop_source();
        ir.blocks[0].operations[0].opcode = opcode;
        for hidden in [false, true] {
            if hidden {
                ir.lifted.as_mut().unwrap().blocks[0].statements.clear();
            }
            let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
            assert!(
                matches!(index.region(id(20), &mut |_| true), Err(Error::Region(_))),
                "opcode {opcode:02x}, hidden={hidden}"
            );
        }
    }
    for terminator in [
        T::Exit {
            kind: crate::ExitKind::Stop,
            values: vec![],
        },
        T::Trap {
            reason: "invalid".into(),
        },
        T::Jump {
            target: id(1),
            edges: vec![id(30)],
            unresolved: false,
            invalid_targets: vec![U256::from(99)],
        },
        T::Jump {
            target: id(1),
            edges: vec![],
            unresolved: false,
            invalid_targets: vec![],
        },
        T::Branch {
            condition: id(1),
            target: id(1),
            then_edges: vec![id(30)],
            else_edge: None,
            unresolved: false,
            invalid_targets: vec![],
        },
        T::Branch {
            condition: id(1),
            target: id(1),
            then_edges: vec![],
            else_edge: Some(id(30)),
            unresolved: false,
            invalid_targets: vec![],
        },
    ] {
        let mut ir = loop_source();
        ir.blocks[0].terminator = terminator;
        let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
        assert!(matches!(
            index.region(id(20), &mut |_| true),
            Err(Error::Region(_))
        ));
    }
}

#[test]
fn source_checks_are_paid_before_expression_domain_allocation() {
    let ir = expression_source();
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let mut calls = 0;
    assert!(matches!(
        index.arithmetic_region(id(1004), &mut |_| {
            calls += 1;
            false
        }),
        Err(Error::Limit)
    ));
    assert_eq!(calls, 1);
}

#[test]
fn a_forged_transfer_target_cannot_escape_the_source_loop() {
    let mut ir = loop_source();
    ir.values.extend([
        value(
            4,
            V::Parameter {
                block: id(60),
                index: 0,
            },
        ),
        value(120, V::MemoryParameter { block: id(60) }),
        value(121, V::EffectParameter { block: id(60) }),
    ]);
    ir.blocks.push(Block {
        id: id(60),
        source: None,
        parameters: vec![id(4)],
        memory_parameter: id(120),
        effect_parameter: id(121),
        operations: vec![],
        terminator: T::Pending,
    });
    ir.lifted.as_mut().unwrap().blocks.push(LiftedBlock {
        id: id(502),
        block: id(60),
        parameters: vec![id(4)],
        statements: vec![],
        condition: None,
        target: None,
        exit_values: vec![],
    });
    ir.edges[0].to = id(60);
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    assert!(matches!(
        index.region(id(20), &mut |_| true),
        Err(Error::Region(_))
    ));
}

#[test]
fn retained_push_immediates_are_compared_before_live_outputs_can_be_shared() {
    let mut ir = loop_source();
    ir.values.iter_mut().find(|v| v.id == id(2)).unwrap().kind = V::Constant(U256::from(5));
    let op = &mut ir.blocks[0].operations[0];
    op.opcode = 0x60;
    op.name = "PUSH1".into();
    op.inputs.clear();
    ir.lifted.as_mut().unwrap().blocks[0].statements[0]
        .inputs
        .clear();
    let mut actual = ir.blocks[0].clone();
    actual.id = id(60);
    actual.parameters = vec![id(6)];
    actual.memory_parameter = id(160);
    actual.effect_parameter = id(161);
    actual.operations[0].id = id(260);
    actual.operations[0].outputs = vec![id(7)];
    actual.terminator = T::Fallthrough { edge: id(62) };
    ir.blocks.push(actual);
    ir.values.extend([
        value(
            6,
            V::Parameter {
                block: id(60),
                index: 0,
            },
        ),
        value(7, V::Constant(U256::from(9))),
        value(160, V::MemoryParameter { block: id(60) }),
        value(161, V::EffectParameter { block: id(60) }),
    ]);
    ir.edges.push(Edge {
        id: id(62),
        from: id(60),
        to: id(60),
        kind: EdgeKind::Jump,
        arguments: vec![id(7)],
        memory: id(160),
        effect: id(161),
    });
    let p = ir.lifted.as_mut().unwrap();
    p.expressions.push(LiftedExpression {
        id: id(1102),
        block: id(60),
        ty: crate::LiftedType::Word256,
        kind: X::Value(id(7)),
    });
    p.blocks.push(LiftedBlock {
        id: id(560),
        block: id(60),
        parameters: vec![id(6)],
        statements: vec![LiftedStatement {
            id: id(360),
            operation: id(260),
            inputs: vec![],
            outputs: vec![id(7)],
        }],
        condition: None,
        target: None,
        exit_values: vec![],
    });
    p.transfers.push(LiftedTransfer {
        id: id(562),
        edge: id(62),
        arguments: vec![id(1102)],
    });
    let K::Sequence(children) = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    let K::While { header, body, .. } = &mut children[1].kind else {
        unreachable!()
    };
    header.kind = K::Block { block: id(60) };
    let K::Sequence(children) = &mut body.kind else {
        unreachable!()
    };
    children[0].kind = K::Transfer { edge: id(62) };
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let canonical = index.region(id(21), &mut |_| true).unwrap();
    let actual = index.region(id(41), &mut |_| true).unwrap();
    assert_eq!(canonical.interface.outputs, vec![id(2)]);
    assert_eq!(actual.interface.outputs, vec![id(7)]);
    assert!(matches!(
        index.correspondence(&canonical, &actual, &mut |_| true),
        Err(Error::Correspondence(_))
    ));
    let bindings = [
        (1, 6),
        (2, 7),
        (10, 60),
        (21, 41),
        (100, 160),
        (101, 161),
        (200, 260),
        (300, 360),
        (500, 560),
    ]
    .into_iter()
    .map(|(formal, actual)| SharedBinding {
        formal: id(formal),
        actual: id(actual),
    })
    .collect();
    let mut supplied = ir.lifted.clone().unwrap();
    supplied.procedures.push(SharedProcedure {
        id: id(9000),
        kind: canonical.kind,
        body: canonical.root,
        interface: canonical.interface,
    });
    supplied.invocations = vec![
        index
            .invocation(id(9001), id(9000), id(21), vec![])
            .unwrap(),
        index
            .invocation(id(9002), id(9000), id(41), bindings)
            .unwrap(),
    ];
    supplied.validate().unwrap();
    assert!(matches!(
        supplied.validate_shared_sources(&ir, |_| true),
        Err(Error::Correspondence(_))
    ));
    ir.values.iter_mut().find(|v| v.id == id(7)).unwrap().kind = V::Constant(U256::from(5));
    let index = SharedSourceIndex::new(&ir, &mut |_| true).unwrap();
    let canonical = index.region(id(21), &mut |_| true).unwrap();
    let actual = index.region(id(41), &mut |_| true).unwrap();
    let bindings = index
        .correspondence(&canonical, &actual, &mut |_| true)
        .unwrap();
    index
        .verify_correspondence(&canonical, &actual, &bindings, &mut |_| true)
        .unwrap();
}
