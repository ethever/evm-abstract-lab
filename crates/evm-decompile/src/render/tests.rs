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
    let arithmetic: Vec<_> = high
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| {
            !statement.outputs.is_empty()
                && stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(|operation| {
                        operation.id == statement.operation && operation.opcode == 0x01
                    })
        })
        .collect();
    assert!(!arithmetic.is_empty());
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Complete);
    assert!(result.text.contains(" + ") && result.text.contains("evm.sstore("));
    assert!(!result.text.contains("evm.add("));
    let renderer = high.render_index(&stage.ir);
    for statement in arithmetic {
        let row = renderer
            .render_statement_with_dependencies(statement, false, |_| true)
            .unwrap();
        assert!(row.starts_with(&format!("v{} = (", statement.outputs[0].0)));
        assert!(row.contains(" + "), "{row}");
        assert_eq!(result.text.matches(&row).count(), 1, "{row}");
    }
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
fn high_export_inverts_empty_then_before_a_nested_condition() {
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
    assert!(!result.text.contains("} else {"), "{}", result.text);
    assert!(result.text.contains("if (v999 == 0) {"), "{}", result.text);
    assert!(result.text.contains("if (v998 != 0) {"), "{}", result.text);
    assert!(result.text.ends_with("    }\n}\n"), "{}", result.text);
}

#[test]
fn real_empty_then_layout_complements_word_truth_without_moving_its_read() {
    use crate::{ExecutionLimits, MemoryHost, U256, execute_bytecode, execute_structured};

    for source_iszero in [false, true] {
        // Both paths meet at STOP. The non-jumping path writes slot zero;
        // the jumping path has only the original empty logical edge transfer.
        let mut bytes = vec![0x5f, 0x35];
        if source_iszero {
            bytes.push(0x15);
        }
        bytes.extend([0x60, if source_iszero { 10 } else { 9 }, 0x57]);
        bytes.extend([0x60, 7, 0x5f, 0x55, 0x5b, 0x00]);
        let input = Input::new(bytes, Fork::Osaka);
        let artifact = decompile(&input, &Config::default());
        assert_eq!(artifact.completion, Completion::Complete);
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let load = stage
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .find(|op| op.opcode == 0x35)
            .unwrap()
            .outputs[0];
        let result = export_text(&artifact, &Cancellation::default());
        assert_eq!(result.status, TextExportStatus::Complete);
        let predicate = format!(
            "if ((v{} {} 0)) {{",
            load.0,
            if source_iszero { "!=" } else { "==" }
        );
        assert!(result.text.contains(&predicate), "{}", result.text);
        assert!(!result.text.contains("} else {"), "{}", result.text);
        assert_eq!(result.text.matches("evm.calldataload(").count(), 1);
        assert_eq!(result.text.matches("evm.sstore(").count(), 1);
        assert!(result.text.find("evm.calldataload(") < result.text.find(&predicate));
        for value in [U256::ZERO, U256::from(1), U256::MAX] {
            let mut raw_host = MemoryHost::new(&input, value.to_be_bytes::<32>().to_vec());
            let mut syntax_host = raw_host.clone();
            let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
            let syntax =
                execute_structured(stage, &mut syntax_host, &ExecutionLimits::default()).unwrap();
            assert_eq!(raw.outcome, syntax.outcome);
            assert_eq!(raw.observations(), syntax.observations());
            assert_eq!(raw_host, syntax_host);
            let stores = if source_iszero {
                value != U256::ZERO
            } else {
                value == U256::ZERO
            };
            assert_eq!(
                raw_host.storage.get(&U256::ZERO).copied(),
                stores.then_some(U256::from(7))
            );
        }
        assert_eq!(
            artifact, original,
            "presentation must not mutate the executable witness"
        );
    }
}

#[test]
fn require_formatting_refusal_stops_before_any_absorbed_suffix_can_resume() {
    let base = decompile(
        &Input::new(
            vec![
                0x34, 0x60, 7, 0x57, 0x60, 9, 0x56, 0x5b, 0x00, 0x5b, 0x5f, 0x5f, 0xfd,
            ],
            Fork::Osaka,
        ),
        &Config::default(),
    );
    for has_data in [false, true] {
        let mut artifact = base.clone();
        let stage = artifact.stages.last_mut().unwrap();
        let source = stage.ir.structured.clone();
        let raw = stage.ir.blocks.clone();
        let boundary = stage.ir.structured.as_ref().unwrap().root.id;
        let program = stage.ir.lifted.as_mut().unwrap();
        if !has_data {
            program.revert_data.clear();
        }
        let control = program
            .controls
            .iter()
            .find(|c| c.guard.is_some())
            .unwrap()
            .clone();
        // Legacy empty guards admit this topology and refuse at expression
        // expansion. Data-bearing snapshots reject the malformed DAG earlier in
        // source admission. Neither path may resume an absorbed failure suffix.
        program
            .expressions
            .iter_mut()
            .find(|e| e.id == control.condition)
            .unwrap()
            .kind = crate::LiftedExpressionKind::Not(control.condition);
        let index = stage.ir.lifted.as_ref().unwrap().render_index(&stage.ir);
        let plan = index.if_plan(control.node, boundary);
        if has_data {
            assert!(index.validate_guard_sources().is_err());
            assert!(!matches!(plan, crate::LiftedIfPlan::Require(_)));
        } else {
            assert!(matches!(plan, crate::LiftedIfPlan::Require(_)));
            assert!(matches!(
                plan.suffix(),
                crate::LiftedIfSuffix::Absorb { .. }
            ));
        }
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
}

#[test]
fn export_requires_consistent_execution_premises_before_publishing_semantic_code() {
    let input = Input::new(
        evm_bytecode::parse_hex(include_str!(
            "../../tests/fixtures/error-payloads/pair-returndata-allocation-guard.hex"
        ))
        .unwrap(),
        Fork::Osaka,
    );
    let artifact = decompile(
        &input,
        &Config {
            execution_domain: crate::ExecutionDomain::EthereumTransactions,
            ..Default::default()
        },
    );
    assert!(artifact.admits_schema());
    assert!(
        !artifact
            .current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .reverts
            .is_empty()
    );
    for mutation in ["config", "stage", "fork", "schema"] {
        let mut changed = artifact.clone();
        match mutation {
            "config" => changed.config.execution_domain = crate::ExecutionDomain::Generic,
            "stage" => {
                let stage = changed.stages.last_mut().unwrap();
                stage.ir.execution_domain = crate::ExecutionDomain::Generic;
                stage.ir.execution_fork = None;
            }
            "fork" => changed.input.fork = Fork::Cancun,
            "schema" => changed.schema_version = crate::SCHEMA_VERSION + 1,
            _ => unreachable!(),
        }
        let result = export_text(&changed, &Cancellation::default());
        assert_eq!(result.status, TextExportStatus::Limited, "{mutation}");
        assert!(
            result
                .text
                .contains("// invalid artifact schema or execution scope")
        );
        assert!(
            !result.text.contains("evm.") && !result.text.contains("require("),
            "{mutation}: {}",
            result.text
        );
    }
}

#[test]
fn export_scope_admission_is_paid_and_cancellation_takes_priority_over_invalid_metadata() {
    let mut artifact = decompile(&Input::new(vec![0x00], Fork::Osaka), &Config::default());
    artifact.stages[0].ir.execution_domain = crate::ExecutionDomain::EthereumTransactions;
    artifact.stages[0].ir.execution_fork = Some(Fork::Osaka);
    let refused = export_text_with_observer(&artifact, &Cancellation::default(), 0, |_| {});
    assert_eq!(refused.status, TextExportStatus::Limited);
    assert_eq!(refused.work, 0);
    assert!(
        !refused.text.contains("invalid artifact"),
        "work refusal precedes metadata scanning"
    );
    let cancelled = Cancellation::new();
    let result = export_text_with_observer(&artifact, &cancelled, u64::MAX, |_| {
        cancelled.cancel();
    });
    assert_eq!(result.status, TextExportStatus::Cancelled);
    assert!(!result.text.contains("invalid artifact"));
    assert!(!result.text.contains("evm."));
}
