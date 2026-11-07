//! Export must account for expanded semantic expressions independently of the
//! small source program/DAG, including cancellation inside a shared expression.
use super::{export_text, export_text_with_observer};
use crate::{Artifact, Cancellation, StageKind, TextExportStatus};
use crate::{
    Completion, Config, EntityId, Fork, Input, LiftedBlock, LiftedProgram, StructuredKind,
    StructuredNode, StructuredProgram, decompile,
};

fn shared_expression() -> Artifact {
    let mut bytes = vec![0x5f, 0x35];
    for _ in 0..20 {
        bytes.extend([0x80, 0x01]);
    }
    bytes.extend([0x5f, 0x55, 0x00]);
    let artifact = decompile(&Input::new(bytes, Fork::Osaka), &Config::default());
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    artifact
}
#[test]
fn real_shared_dag_keeps_named_computations_and_exports_completely() {
    let artifact = shared_expression();
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        high.blocks
            .iter()
            .flat_map(|block| &block.statements)
            .any(|statement| !statement.outputs.is_empty()
                && stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(
                        |operation| operation.id == statement.operation && operation.opcode == 0x01
                    ))
    );
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Complete);
    assert!(result.text.contains("evm.add(") && result.text.contains("evm.sstore("));
    assert!(!result.text.contains("work limit exhausted"));
    assert_eq!(result.text.matches("evm.calldataload(").count(), 1);
}

#[test]
fn expression_text_ceiling_cannot_report_complete_export() {
    use crate::{
        LiftedExpression, LiftedExpressionKind as Expression, LiftedRenderError, LiftedType,
    };
    let mut artifact = shared_expression();
    let stage = artifact.stages.last_mut().unwrap();
    let block = &stage.ir.blocks[0];
    let load = block
        .operations
        .iter()
        .find(|operation| operation.opcode == 0x35)
        .unwrap();
    let adds: Vec<_> = block
        .operations
        .iter()
        .filter(|operation| operation.opcode == 0x01)
        .collect();
    assert_eq!(adds.len(), 20);
    let mut next = crate::records(&stage.ir)
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap()
        + 1;
    let offset = EntityId(next);
    next += 1;
    let mut value = EntityId(next);
    next += 1;
    let mut expressions = vec![
        LiftedExpression {
            id: offset,
            block: block.id,
            ty: LiftedType::Word256,
            kind: Expression::Word(crate::U256::ZERO),
        },
        LiftedExpression {
            id: value,
            block: block.id,
            ty: LiftedType::Word256,
            kind: Expression::Value(load.outputs[0]),
        },
    ];
    // This presentation-only model bypasses projector materialization. Its
    // twenty real ADDs stay a shared DAG with 2^20 inline leaf occurrences.
    for _ in &adds {
        let id = EntityId(next);
        next += 1;
        expressions.push(LiftedExpression {
            id,
            block: block.id,
            ty: LiftedType::Word256,
            kind: Expression::WordOp {
                opcode: 0x01,
                arguments: vec![value, value],
            },
        });
        value = id;
    }
    let program = stage.ir.lifted.as_mut().unwrap();
    program.expressions = expressions;
    let high = &mut program.blocks[0];
    high.statements.retain(|statement| {
        !adds
            .iter()
            .any(|operation| operation.id == statement.operation)
    });
    for statement in &mut high.statements {
        let operation = block
            .operations
            .iter()
            .find(|operation| operation.id == statement.operation)
            .unwrap();
        statement.inputs = match operation.opcode {
            0x35 => vec![offset],
            0x55 => vec![offset, value],
            0x00 => vec![],
            opcode => panic!("unexpected retained fixture opcode {opcode:#x}"),
        };
    }
    program.validate().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    assert_eq!(
        renderer.render_expression(value, |_| true),
        Err(LiftedRenderError::Limit)
    );
    let store = program.blocks[0]
        .statements
        .iter()
        .find(|statement| {
            block
                .operations
                .iter()
                .any(|operation| operation.id == statement.operation && operation.opcode == 0x55)
        })
        .unwrap();
    assert_eq!(
        renderer.render_statement(store, |_| true),
        Err(LiftedRenderError::Limit)
    );
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Limited);
    assert!(result.work > 4_096);
    assert!(result.text.contains("work limit exhausted"));
    assert!(!result.text.contains("evm.sstore("));
    assert_eq!(result.text.matches("evm.calldataload(").count(), 1);
}
#[test]
fn semantic_expression_expansion_charges_work_and_checks_cancellation() {
    let artifact = shared_expression();
    let cancel = Cancellation::new();
    let mut checkpoints = 0;
    let result = export_text_with_observer(&artifact, &cancel, u64::MAX, |_| {
        checkpoints += 1;
        if checkpoints == 64 {
            cancel.cancel();
        }
    });
    assert!(cancel.is_cancelled());
    assert_eq!(result.status, TextExportStatus::Cancelled);
    assert!(
        result.work > 0 && result.work < u64::try_from(crate::MAX_LIFTED_EXPRESSION_TEXT).unwrap()
    );
    let result = export_text_with_observer(&artifact, &Cancellation::new(), 2_048, |_| {});
    assert_eq!(result.status, TextExportStatus::Limited);
    assert!(result.work <= 2_048);
}

fn node(id: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode {
        id: EntityId(id),
        kind,
    }
}

// Isolated presentation fixture: the empty block exercises the renderer's
// content summary, rather than claiming a new engine transformation proof.
fn branch_artifact(otherwise: StructuredNode) -> Artifact {
    let mut artifact = decompile(&Input::new(vec![0x00], Fork::Osaka), &Config::default());
    let stage = artifact.stages.last_mut().unwrap();
    let block = stage.ir.blocks[0].id;
    stage.ir.lifted = Some(LiftedProgram {
        blocks: vec![LiftedBlock {
            id: EntityId(101),
            block,
            parameters: vec![],
            statements: vec![],
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        ..LiftedProgram::default()
    });
    stage.ir.structured = Some(StructuredProgram {
        root: node(
            200,
            StructuredKind::If {
                condition: EntityId(999),
                then_branch: Box::new(node(201, StructuredKind::Sequence(vec![]))),
                else_branch: Box::new(otherwise),
            },
        ),
    });
    artifact
}

#[test]
fn high_export_omits_an_empty_block_else_and_keeps_the_whole_if() {
    let mut artifact = branch_artifact(node(202, StructuredKind::Sequence(vec![])));
    let stage = artifact.stages.last_mut().unwrap();
    let block = stage.ir.blocks[0].id;
    let StructuredKind::If { else_branch, .. } =
        &mut stage.ir.structured.as_mut().unwrap().root.kind
    else {
        unreachable!()
    };
    **else_branch = node(202, StructuredKind::Block { block });
    let original = artifact.clone();
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Complete);
    assert_eq!(result.text.matches("if (").count(), 1, "{}", result.text);
    assert!(!result.text.contains("} else {"), "{}", result.text);
    assert!(result.text.ends_with("{\n}\n"), "{}", result.text);
    assert_eq!(artifact, original);
}

#[test]
fn high_export_retains_an_empty_nested_if_as_else_content() {
    let artifact = branch_artifact(node(
        202,
        StructuredKind::If {
            condition: EntityId(998),
            then_branch: Box::new(node(203, StructuredKind::Sequence(vec![]))),
            else_branch: Box::new(node(204, StructuredKind::Sequence(vec![]))),
        },
    ));
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Complete);
    assert_eq!(result.text.matches("if (").count(), 2, "{}", result.text);
    assert_eq!(
        result.text.matches("} else {").count(),
        1,
        "{}",
        result.text
    );
    assert!(result.text.ends_with("    }\n}\n"), "{}", result.text);
}

#[test]
fn require_formatting_refusal_stops_before_any_absorbed_suffix_can_resume() {
    let mut artifact = decompile(
        &Input::new(
            vec![
                0x34, 0x60, 7, 0x57, 0x60, 9, 0x56, 0x5b, 0x00, 0x5b, 0x5f, 0x5f, 0xfd,
            ],
            Fork::Osaka,
        ),
        &Config::default(),
    );
    let stage = artifact.stages.last_mut().unwrap();
    let source = stage.ir.structured.clone();
    let raw = stage.ir.blocks.clone();
    let boundary = stage.ir.structured.as_ref().unwrap().root.id;
    let program = stage.ir.lifted.as_mut().unwrap();
    let control = program
        .controls
        .iter()
        .find(|c| c.guard.is_some())
        .unwrap()
        .clone();
    // Topology remains admitted, but expression expansion must refuse this
    // malformed cyclic diagnostic artifact before publishing a require row.
    program
        .expressions
        .iter_mut()
        .find(|e| e.id == control.condition)
        .unwrap()
        .kind = crate::LiftedExpressionKind::Not(control.condition);
    let index = stage.ir.lifted.as_ref().unwrap().render_index(&stage.ir);
    let plan = index.if_plan(control.node, boundary);
    assert!(matches!(plan, crate::LiftedIfPlan::Require(_)));
    assert!(matches!(
        plan.suffix(),
        crate::LiftedIfSuffix::Absorb { .. }
    ));
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Limited);
    assert!(
        !result.text.contains("require(") && !result.text.contains("if ("),
        "{}",
        result.text
    );
    assert!(
        !result.text.contains("evm.revert(") && !result.text.contains("evm.stop("),
        "{}",
        result.text
    );
    assert_eq!(artifact.current().unwrap().ir.blocks, raw);
    assert_eq!(artifact.current().unwrap().ir.structured, source);
}
