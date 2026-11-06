//! Source topology is admitted separately from core's semantic failure proof.
use evm_decompile_model::*;

fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn node(n: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode { id: id(n), kind }
}
fn fixture() -> Ir {
    let block = |source, terminator| Block {
        id: id(source),
        source: None,
        parameters: vec![],
        memory_parameter: id(source + 3),
        effect_parameter: id(source + 4),
        operations: vec![],
        terminator,
    };
    let high = |n, source, condition, exit_values| LiftedBlock {
        id: id(n),
        block: id(source),
        parameters: vec![],
        statements: vec![],
        condition,
        target: None,
        exit_values,
    };
    let pass = node(
        301,
        StructuredKind::Sequence(vec![
            node(304, StructuredKind::Transfer { edge: id(200) }),
            node(305, StructuredKind::Block { block: id(110) }),
            node(306, StructuredKind::Exit { block: id(110) }),
        ]),
    );
    Ir {
        blocks: vec![
            block(
                100,
                Terminator::Branch {
                    condition: id(101),
                    target: id(102),
                    then_edges: vec![id(200)],
                    else_edge: Some(id(201)),
                    unresolved: false,
                    invalid_targets: vec![],
                },
            ),
            block(
                110,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
            ),
            block(
                120,
                Terminator::Exit {
                    kind: ExitKind::Revert,
                    values: vec![id(122), id(122)],
                },
            ),
        ],
        values: vec![Value {
            id: id(101),
            kind: ValueKind::Parameter {
                block: id(100),
                index: 0,
            },
            fact: ValueFact::Unknown,
        }],
        edges: [(200, 110, EdgeKind::True), (201, 120, EdgeKind::False)]
            .into_iter()
            .map(|(n, to, kind)| Edge {
                id: id(n),
                from: id(100),
                to: id(to),
                kind,
                arguments: vec![],
                memory: id(103),
                effect: id(104),
            })
            .collect(),
        structured: Some(StructuredProgram {
            root: node(
                303,
                StructuredKind::Sequence(vec![
                    node(307, StructuredKind::Block { block: id(100) }),
                    node(
                        300,
                        StructuredKind::If {
                            condition: id(101),
                            then_branch: Box::new(pass),
                            else_branch: Box::new(node(
                                302,
                                StructuredKind::Transfer { edge: id(201) },
                            )),
                        },
                    ),
                    node(308, StructuredKind::Block { block: id(120) }),
                    node(309, StructuredKind::Exit { block: id(120) }),
                ]),
            ),
        }),
        lifted: Some(LiftedProgram {
            expressions: vec![
                LiftedExpression {
                    id: id(1),
                    block: id(100),
                    ty: LiftedType::Predicate,
                    kind: LiftedExpressionKind::Predicate(true),
                },
                LiftedExpression {
                    id: id(2),
                    block: id(120),
                    ty: LiftedType::Word256,
                    kind: LiftedExpressionKind::Word(U256::ZERO),
                },
            ],
            blocks: vec![
                high(3, 100, Some(id(1)), vec![]),
                high(4, 110, None, vec![]),
                high(5, 120, None, vec![id(2), id(2)]),
            ],
            transfers: vec![
                LiftedTransfer {
                    id: id(6),
                    edge: id(200),
                    arguments: vec![],
                },
                LiftedTransfer {
                    id: id(7),
                    edge: id(201),
                    arguments: vec![],
                },
            ],
            controls: vec![LiftedControl {
                id: id(8),
                node: id(300),
                condition: id(1),
                guard: Some(LiftedGuard {
                    block: id(100),
                    pass_nonzero: true,
                    pass_edge: id(200),
                    failure_edge: id(201),
                    pass_node: id(301),
                    failure_node: id(302),
                    payload: LiftedGuardPayload::EmptyRevert,
                    continuation: Some(LiftedGuardContinuation {
                        sequence: id(303),
                        start: 2,
                        end: 4,
                        pass_falls_through: false,
                    }),
                }),
            }],
            failures: vec![LiftedFailure {
                block: id(120),
                rank: 0,
            }],
        }),
        ..Ir::default()
    }
}
fn guard(ir: &mut Ir) -> &mut LiftedGuard {
    ir.lifted.as_mut().unwrap().controls[0]
        .guard
        .as_mut()
        .unwrap()
}
fn refused(ir: &Ir) {
    let program = ir.lifted.as_ref().unwrap();
    let index = program.render_index(ir); // Deliberately bypass model/source admission.
    assert!(matches!(
        program.validate_guard_sources(ir),
        Err(LiftedModelError::GuardSource(_))
    ));
    assert!(matches!(
        index.validate_guard_sources(),
        Err(LiftedRenderError::InvalidGuardSource(_))
    ));
    assert!(index.guard(id(300)).is_none());
    assert!(index.control(id(300)).is_none());
    assert_eq!(index.sequence_end(id(303), 2), None);
    assert!(matches!(
        index.render_guard(&program.controls[0], |_| true),
        Err(LiftedRenderError::InvalidGuardSource(_))
    ));
    assert!(
        index.node(id(300)).is_some(),
        "raw witness is still inspectable"
    );
    assert_eq!(
        program.render_expression(id(1)),
        "true",
        "expression-only formatting needs no IR"
    );
}

#[test]
fn valid_source_plan_and_expression_only_formatting_remain_available() {
    let ir = fixture();
    let program = ir.lifted.as_ref().unwrap();
    program.validate().unwrap();
    program.validate_guard_sources(&ir).unwrap();
    let index = program.render_index(&ir);
    assert_eq!(index.validate_guard_sources(), Ok(()));
    assert_eq!(index.guard(id(300)).unwrap().pass_node, id(301));
    assert_eq!(index.sequence_end(id(303), 2), Some(4));
    assert_eq!(
        index
            .render_guard(&program.controls[0], |_| true)
            .unwrap()
            .as_deref(),
        Some("require(true);")
    );
}

#[test]
fn guard_cannot_target_itself_ancestor_sibling_other_arm_or_out_of_tree_node() {
    for target in [300, 303, 308, 302, 999] {
        let mut ir = fixture();
        guard(&mut ir).pass_node = id(target);
        refused(&ir);
    }
    let mut ir = fixture();
    guard(&mut ir).failure_node = id(301);
    refused(&ir);
    let mut ir = fixture();
    guard(&mut ir).pass_nonzero = false;
    refused(&ir);
}

#[test]
fn only_an_exact_immediate_suffix_with_raw_pass_fallthrough_can_be_omitted() {
    for field in ["scope", "start", "end", "fallthrough"] {
        let mut ir = fixture();
        let tail = guard(&mut ir).continuation.as_mut().unwrap();
        match field {
            "scope" => tail.sequence = id(301),
            "start" => tail.start += 1,
            "end" => tail.end -= 1,
            "fallthrough" => tail.pass_falls_through = true,
            _ => unreachable!(),
        }
        refused(&ir);
    }
    let mut ir = fixture();
    let StructuredKind::Sequence(children) = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    let StructuredKind::If { then_branch, .. } = &mut children[1].kind else {
        unreachable!()
    };
    let StructuredKind::Sequence(pass) = &mut then_branch.kind else {
        unreachable!()
    };
    pass[2].kind = StructuredKind::Transfer { edge: id(202) };
    ir.edges.push(Edge {
        id: id(202),
        from: id(110),
        to: id(120),
        kind: EdgeKind::Jump,
        arguments: vec![],
        memory: id(113),
        effect: id(114),
    });
    ir.blocks[1].terminator = Terminator::Fallthrough { edge: id(202) };
    guard(&mut ir)
        .continuation
        .as_mut()
        .unwrap()
        .pass_falls_through = true;
    let program = ir.lifted.as_ref().unwrap();
    program.validate_guard_sources(&ir).unwrap();
    assert_eq!(program.render_index(&ir).sequence_end(id(303), 2), None);
    guard(&mut ir)
        .continuation
        .as_mut()
        .unwrap()
        .pass_falls_through = false;
    refused(&ir); // A successful fragment still reaches the join REVERT.
}

#[test]
fn raw_branch_predicate_edges_and_owned_tree_identities_are_checked() {
    for field in [
        "condition",
        "source_node",
        "block",
        "edge",
        "raw_condition",
        "duplicate_ast",
    ] {
        let mut ir = fixture();
        match field {
            "condition" => ir.lifted.as_mut().unwrap().controls[0].condition = id(2),
            "source_node" => ir.lifted.as_mut().unwrap().controls[0].node = id(307),
            "block" => guard(&mut ir).block = id(110),
            "edge" => guard(&mut ir).pass_edge = id(201),
            "raw_condition" => {
                let Terminator::Branch { condition, .. } = &mut ir.blocks[0].terminator else {
                    unreachable!()
                };
                *condition = id(102);
            }
            "duplicate_ast" => {
                let StructuredKind::Sequence(children) =
                    &mut ir.structured.as_mut().unwrap().root.kind
                else {
                    unreachable!()
                };
                children[2].id = id(300);
            }
            _ => unreachable!(),
        }
        refused(&ir);
    }
}

#[test]
fn formatter_rejects_forged_controls_and_quarantine_removes_prior_omissions() {
    let mut ir = fixture();
    let program = ir.lifted.as_ref().unwrap();
    let index = program.render_index(&ir);
    let mut forged = program.controls[0].clone();
    forged.guard.as_mut().unwrap().pass_node = forged.node;
    assert!(matches!(
        index.render_guard(&forged, |_| true),
        Err(LiftedRenderError::InvalidGuardSource(_))
    ));
    let mut additional = program.controls[0].clone();
    additional.id = id(20);
    additional.node = id(999);
    ir.lifted.as_mut().unwrap().controls.push(additional);
    refused(&ir);
}

#[test]
fn legacy_guard_free_projection_does_not_require_guard_source_witnesses() {
    let mut ir = fixture();
    let program = ir.lifted.as_mut().unwrap();
    program.controls[0].guard = None;
    program.failures.clear();
    let old = serde_json::to_string(program).unwrap();
    ir.structured = None;
    let program = ir.lifted.as_ref().unwrap();
    program.validate_guard_sources(&ir).unwrap();
    assert_eq!(program.render_index(&ir).validate_guard_sources(), Ok(()));
    assert_eq!(serde_json::to_string(program).unwrap(), old);
}
