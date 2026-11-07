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
fn expression_text_ceiling_cannot_report_complete_export() {
    let artifact = shared_expression();
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
