//! Ordinary branch plans are source metadata, prepared before formatting rows.
use evm_decompile_model::{
    Block, Edge, EdgeKind, EntityId, ExitKind, Ir, LiftedBlock, LiftedBranchLayout, LiftedControl,
    LiftedExpression, LiftedExpressionKind as Expression, LiftedProgram, LiftedRenderError,
    LiftedStatement, LiftedTransfer, LiftedType, StructuredKind as Kind, StructuredNode,
    StructuredProgram, Terminator,
};

fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn node(n: u64, kind: Kind) -> StructuredNode {
    StructuredNode { id: id(n), kind }
}

// Renderer-only source: no claim of an engine transformation or execution proof.
fn fixture() -> Ir {
    let raw = |n| Block {
        id: id(n),
        source: None,
        parameters: vec![],
        memory_parameter: id(1000 + n),
        effect_parameter: id(2000 + n),
        operations: vec![],
        terminator: Terminator::Exit {
            kind: ExitKind::Stop,
            values: vec![],
        },
    };
    let high = |n| LiftedBlock {
        id: id(300 + n),
        block: id(n),
        parameters: vec![],
        statements: vec![],
        condition: None,
        target: None,
        exit_values: vec![],
    };
    Ir {
        entry: Some(id(1)),
        blocks: vec![raw(1), raw(2), raw(3)],
        edges: [(201, 2, EdgeKind::True), (202, 3, EdgeKind::False)]
            .into_iter()
            .map(|(n, to, kind)| Edge {
                id: id(n),
                from: id(1),
                to: id(to),
                kind,
                arguments: vec![],
                memory: id(1001),
                effect: id(2001),
            })
            .collect(),
        structured: Some(StructuredProgram {
            root: node(
                100,
                Kind::If {
                    condition: id(30),
                    then_branch: Box::new(node(102, Kind::Block { block: id(2) })),
                    else_branch: Box::new(node(103, Kind::Block { block: id(3) })),
                },
            ),
        }),
        lifted: Some(LiftedProgram {
            blocks: vec![high(1), high(2), high(3)],
            expressions: vec![
                LiftedExpression {
                    id: id(501),
                    block: id(1),
                    ty: LiftedType::Word256,
                    kind: Expression::Value(id(30)),
                },
                LiftedExpression {
                    id: id(502),
                    block: id(1),
                    ty: LiftedType::Predicate,
                    kind: Expression::NonZero(id(501)),
                },
            ],
            controls: vec![LiftedControl {
                id: id(503),
                node: id(100),
                condition: id(502),
                guard: None,
            }],
            ..LiftedProgram::default()
        }),
        ..Ir::default()
    }
}

fn content(ir: &mut Ir, block: usize) {
    ir.lifted.as_mut().unwrap().blocks[block]
        .statements
        .push(LiftedStatement {
            id: id(600 + block as u64),
            operation: id(700 + block as u64),
            inputs: vec![],
            outputs: vec![],
        });
}
fn layout(ir: &Ir) -> LiftedBranchLayout {
    ir.lifted
        .as_ref()
        .unwrap()
        .render_base_index_with_budget(ir, |_| true)
        .plan_branch_layouts(ir.structured.as_ref().unwrap().root.id, |_| false, |_| true)
        .unwrap()
        .get(id(100))
        .copied()
        .unwrap()
}

#[test]
fn all_arm_combinations_keep_original_ids_and_complement_a_typed_predicate_only() {
    for (left, right, invert, then_id, else_id) in [
        (false, false, false, 102, None),
        (true, false, false, 102, None),
        (false, true, true, 103, None),
        (true, true, false, 102, Some(103)),
    ] {
        let mut ir = fixture();
        if left {
            content(&mut ir, 1);
        }
        if right {
            content(&mut ir, 2);
        }
        let before = serde_json::to_vec(&ir).unwrap();
        let selected = layout(&ir);
        assert_eq!(
            selected,
            LiftedBranchLayout {
                invert,
                then_node: id(then_id),
                else_node: else_id.map(id)
            }
        );
        let renderer = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert_eq!(
            renderer
                .render_condition(id(502), selected.invert, |_| true)
                .unwrap(),
            if invert { "(v30 == 0)" } else { "(v30 != 0)" }
        );
        assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
    }
}

#[test]
fn live_scalar_tuples_are_content_but_dependency_tokens_do_not_change_layout() {
    let mut ir = fixture();
    let Kind::If {
        then_branch,
        else_branch,
        ..
    } = &mut ir.structured.as_mut().unwrap().root.kind
    else {
        unreachable!()
    };
    then_branch.kind = Kind::Transfer { edge: id(201) };
    else_branch.kind = Kind::Transfer { edge: id(202) };
    ir.lifted.as_mut().unwrap().transfers = vec![
        LiftedTransfer {
            id: id(801),
            edge: id(201),
            arguments: vec![],
        },
        LiftedTransfer {
            id: id(802),
            edge: id(202),
            arguments: vec![id(501)],
        },
    ];
    assert_eq!(
        layout(&ir),
        LiftedBranchLayout {
            invert: true,
            then_node: id(103),
            else_node: None
        }
    );
    ir.edges[0].memory = id(99999);
    ir.edges[0].effect = id(99998);
    assert!(layout(&ir).invert);
    ir.lifted.as_mut().unwrap().transfers[0]
        .arguments
        .push(id(501));
    assert_eq!(
        layout(&ir),
        LiftedBranchLayout {
            invert: false,
            then_node: id(102),
            else_node: Some(id(103))
        }
    );
}

#[test]
fn nested_empty_controls_and_unknown_witnesses_remain_content() {
    let mut ir = fixture();
    content(&mut ir, 2);
    let Kind::If { then_branch, .. } = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    then_branch.kind = Kind::If {
        condition: id(30),
        then_branch: Box::new(node(110, Kind::Sequence(vec![]))),
        else_branch: Box::new(node(111, Kind::Sequence(vec![]))),
    };
    assert!(
        !layout(&ir).invert,
        "an inner predicate remains even with two empty arms"
    );
    let mut ir = fixture();
    content(&mut ir, 2);
    ir.lifted.as_mut().unwrap().blocks.remove(1);
    assert!(
        !layout(&ir).invert,
        "missing block metadata must retain diagnostic/raw fallback"
    );
    let Kind::If { then_branch, .. } = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    then_branch.kind = Kind::Transfer { edge: id(201) };
    assert!(
        !layout(&ir).invert,
        "a missing live-transfer witness cannot certify an empty arm"
    );
}

#[test]
fn empty_high_metadata_without_its_raw_block_cannot_omit_a_source_arm() {
    for missing in [1, 2] {
        let mut ir = fixture();
        content(&mut ir, if missing == 1 { 2 } else { 1 });
        let absent = ir.blocks.remove(missing).id;
        assert!(
            ir.lifted
                .as_ref()
                .unwrap()
                .blocks
                .iter()
                .any(|b| b.block == absent)
        );
        assert_eq!(
            layout(&ir),
            LiftedBranchLayout {
                invert: false,
                then_node: id(102),
                else_node: Some(id(103)),
            }
        );
    }
}

#[test]
fn an_empty_transfer_without_its_raw_edge_cannot_certify_an_empty_arm() {
    let mut ir = fixture();
    content(&mut ir, 2);
    let Kind::If { then_branch, .. } = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    then_branch.kind = Kind::Transfer { edge: id(201) };
    ir.lifted.as_mut().unwrap().transfers.push(LiftedTransfer {
        id: id(801),
        edge: id(201),
        arguments: vec![],
    });
    ir.edges.retain(|edge| edge.id != id(201));
    assert_eq!(
        layout(&ir),
        LiftedBranchLayout {
            invert: false,
            then_node: id(102),
            else_node: Some(id(103)),
        }
    );
}

#[test]
fn actual_call_projection_and_canonical_root_expansion_are_explicit_inputs() {
    let mut ir = fixture();
    content(&mut ir, 2);
    let program = ir.lifted.as_ref().unwrap();
    let renderer = program.render_base_index_with_budget(&ir, |_| true);
    // A browser base index has no shared admission. The already prepared caller
    // still reports a real zero-result call at this otherwise empty source arm.
    let calls = renderer
        .plan_branch_layouts(id(100), |source| source == id(102), |_| true)
        .unwrap();
    assert_eq!(calls.get(id(100)).unwrap().else_node, Some(id(103)));
    let expanded = renderer
        .plan_branch_layouts(id(100), |_| false, |_| true)
        .unwrap();
    assert!(expanded.get(id(100)).unwrap().invert);
    let root_call = renderer
        .plan_branch_layouts(id(100), |source| source == id(100), |_| true)
        .unwrap();
    assert!(root_call.get(id(100)).is_none());
    let definition = renderer
        .plan_branch_layouts(
            id(100),
            |source| source != id(100) && source == id(102),
            |_| true,
        )
        .unwrap();
    assert!(
        definition.get(id(100)).is_some(),
        "canonical definition expands its own invocation root"
    );
}

#[test]
fn preparation_refusal_is_atomic_and_each_nested_source_is_visited_once() {
    let mut ir = fixture();
    let mut tree = node(10000, Kind::Sequence(vec![]));
    let depth = 128_u64;
    for level in 0..depth {
        tree = node(
            20000 + level,
            Kind::If {
                condition: id(30),
                then_branch: Box::new(tree),
                else_branch: Box::new(node(30000 + level, Kind::Sequence(vec![]))),
            },
        );
    }
    ir.structured.as_mut().unwrap().root = tree;
    let renderer = ir
        .lifted
        .as_ref()
        .unwrap()
        .render_base_index_with_budget(&ir, |_| true);
    let boundary = ir.structured.as_ref().unwrap().root.id;
    let mut visited = 0;
    let mut work = 0;
    renderer
        .plan_branch_layouts(
            boundary,
            |_| {
                visited += 1;
                false
            },
            |amount| {
                work += amount;
                true
            },
        )
        .unwrap();
    assert_eq!(visited, 2 * depth + 1);
    assert!(
        work < 200 * (2 * depth + 1) as usize,
        "work {work} must scale with nodes, not ancestor pairs"
    );
    for allowance in [0, 1, 8, 128] {
        let mut calls = 0;
        let result = renderer.plan_branch_layouts(
            boundary,
            |_| false,
            |_| {
                calls += 1;
                calls <= allowance
            },
        );
        assert!(matches!(result, Err(LiftedRenderError::Limit)));
    }
}
