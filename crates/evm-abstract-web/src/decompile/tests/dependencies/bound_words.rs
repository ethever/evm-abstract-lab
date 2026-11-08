//! A retained pure definition keeps its source/SSA identity while all code
//! consumers use the model's simplified typed RHS. Effects remain primitives.
use super::*;
use crate::decompile::structured::HighDisplayOptions;
use evm_abstract_protocol::decompile_model::{LiftedExpressionKind, LiftedStatement, U256};
use guards::{Runtime, rows_with};

const MASK160: U256 = U256::from_limbs([u64::MAX, u64::MAX, 0xffff_ffff, 0]);

fn push_mask(runtime: &mut Runtime, value: U256) {
    let width = if value <= MASK160 { 20 } else { 32 };
    runtime.emit(&[0x5f + u8::try_from(width).unwrap()]);
    runtime.emit(&value.to_be_bytes::<32>()[32 - width..]);
}
fn cross(left: U256, right: U256, memory: bool) -> String {
    if !memory && left == MASK160 && right == MASK160 {
        return "600a5473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16610033565b5f5500".to_owned();
    }
    let mut runtime = Runtime::default();
    if memory {
        runtime.emit(&[0x5f, 0x35, 0x5f, 0x52, 0x5f, 0x51]);
    } else {
        runtime.emit(&[0x60, 0x0a, 0x54]);
    }
    push_mask(&mut runtime, left);
    runtime.emit(&[0x16]);
    push_mask(&mut runtime, right);
    runtime.emit(&[0x16]);
    runtime.jump("store", false);
    runtime.label("store");
    runtime.emit(&[0x5f, 0x55, 0x00]);
    runtime.finish()
}
fn shared_mask(left: U256, right: U256) -> String {
    let mut runtime = Runtime::default();
    runtime.address("first-return");
    runtime.emit(&[0x5f, 0x35]);
    runtime.jump("helper", false);
    runtime.label("first-return");
    runtime.emit(&[0x60, 0x01, 0x55]);
    runtime.address("second-return");
    runtime.emit(&[0x60, 0x20, 0x35]);
    runtime.jump("helper", false);
    runtime.label("second-return");
    runtime.emit(&[0x60, 0x02, 0x55, 0x00]);
    runtime.label("helper");
    runtime.emit(&[0x54]);
    push_mask(&mut runtime, left);
    runtime.emit(&[0x16]);
    push_mask(&mut runtime, right);
    runtime.emit(&[0x16]);
    // Keep the observable helper body with its bound AND so discovery has
    // useful source work to share even when the surrounding Sequence flattens.
    runtime.emit(&[0x80, 0x5f, 0x52, 0x5f, 0x51, 0x50, 0x80, 0x5f, 0x55]);
    runtime.emit(&[0x60, 0x20, 0x5f, 0xa0]);
    runtime.jump("return-value", false);
    runtime.label("return-value");
    runtime.emit(&[0x60, 0x01, 0x01, 0x90, 0x56]);
    runtime.finish()
}
fn operation(stage: &Stage, statement: &LiftedStatement) -> u8 {
    stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| operation.id == statement.operation)
        .unwrap()
        .opcode
}
fn bound<'a>(
    stage: &'a Stage,
    program: &'a evm_abstract_protocol::decompile_model::LiftedProgram,
) -> &'a LiftedStatement {
    program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| operation(stage, statement) == 0x16 && !statement.outputs.is_empty())
        .unwrap()
}
fn assert_single_mask(text: &str, output: EntityId, read: EntityId, mask: U256) {
    let expected = [
        format!("v{} = ({mask:#x} & v{});", output.0, read.0),
        format!("v{} = (v{} & {mask:#x});", output.0, read.0),
    ];
    assert!(expected.iter().any(|expected| expected == text), "{text}");
    assert_eq!(text.matches(&format!("{mask:#x}")).count(), 1);
    assert_eq!(text.matches('&').count(), 1);
    assert!(!text.contains("evm.and("));
}

#[test]
fn retained_mask_rows_keep_source_binding_copy_and_metadata_consistent() {
    for (left, right) in [(MASK160, MASK160), (U256::from(0xf0), U256::from(0xcc))] {
        let artifact = artifact(&cross(left, right, false));
        let original_wire = serde_json::to_vec(&artifact).unwrap();
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let target = bound(stage, program);
        let read = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| operation(stage, statement) == 0x54)
            .unwrap();
        let store = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| operation(stage, statement) == 0x55)
            .unwrap();
        assert_eq!(target.outputs.len(), 1);
        assert_eq!(
            target.inputs.len(),
            2,
            "wire statement remains the original primitive"
        );
        assert_eq!(read.outputs.len(), 1);
        assert_eq!(
            program.expression(store.inputs[1]).unwrap().kind,
            LiftedExpressionKind::Value(target.outputs[0])
        );
        let normal = high(stage, false);
        let target_text = &normal
            .iter()
            .find(|line| line.entity == target.id)
            .unwrap()
            .text;
        assert_single_mask(
            target_text,
            target.outputs[0],
            read.outputs[0],
            left & right,
        );
        let expected_order = [read.id, target.id, store.id];
        let ordered: Vec<_> = normal
            .iter()
            .filter(|line| expected_order.contains(&line.entity))
            .map(|line| line.entity)
            .collect();
        assert_eq!(ordered, expected_order);
        assert_eq!(copied_text(&normal).matches("evm.sload(").count(), 1);
        assert_eq!(copied_text(&normal).matches("evm.sstore(").count(), 1);
        for dependencies in [false, true] {
            for annotations in [false, true] {
                let lines = rows_with(
                    stage,
                    HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    },
                );
                let line = lines.iter().find(|line| line.entity == target.id).unwrap();
                assert_eq!(line.text, *target_text);
                assert_eq!(
                    lines.iter().filter(|line| line.entity == target.id).count(),
                    1
                );
            }
        }
        let (ctx, mut workspace) = setup(artifact.clone());
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&normal)]
        );
        let mut display = (false, false);
        for (dependencies, annotations) in
            [(true, false), (true, true), (false, true), (false, false)]
        {
            if display.0 != dependencies {
                click(&ctx, &mut workspace, "Show state dependencies");
            }
            if display.1 != annotations {
                click(&ctx, &mut workspace, "Show structure annotations");
            }
            display = (dependencies, annotations);
            let expected = rows_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            assert_eq!(
                click(&ctx, &mut workspace, "Copy structured text").copied,
                [copied_text(&expected)]
            );
        }
        let at = EntityRef {
            stage: stage.id,
            entity: target.id,
        };
        let raw = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|operation| operation.id == target.operation)
            .unwrap();
        let source = raw.source.unwrap().pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let selected = frame(&ctx, &mut workspace, vec![]);
        assert!(
            selected
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code"
                    && row["selected"] == true
                    && row["label"].as_str().unwrap().ends_with(target_text))
        );
        click(&ctx, &mut workspace, "Transform inspector");
        let painted_details = labels(
            &frame(&ctx, &mut workspace, vec![]),
            "decompile-entity-detail",
        );
        assert!(
            painted_details
                .iter()
                .any(|line| line.starts_with("Statement") && line.contains(target_text))
        );
        let details = workspace.state.details(&artifact, at);
        assert!(
            details
                .lines
                .iter()
                .any(|line| line.starts_with("Statement") && line.contains(target_text))
        );
        assert!(details.references.contains(&EntityRef {
            stage: stage.id,
            entity: target.operation
        }));
        for input in &target.inputs {
            assert!(details.references.contains(&EntityRef {
                stage: stage.id,
                entity: *input
            }));
        }
        assert_eq!(
            serde_json::to_vec(workspace.artifact.as_ref().unwrap()).unwrap(),
            original_wire
        );
    }
}

#[test]
fn zero_mask_keeps_the_read_and_storage_effect_while_memory_primitives_stay_explicit() {
    let artifact = artifact(&cross(U256::ZERO, MASK160, false));
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let text = copied_text(&high(stage, false));
    assert_eq!(text.matches("evm.sload(").count(), 1);
    assert_eq!(text.matches("evm.sstore(").count(), 1);
    assert!(text.contains("evm.sstore(slot = 0x0, value = 0x0);"));
    let outer = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .rfind(|operation| operation.opcode == 0x16)
        .unwrap();
    assert_eq!(outer.replacement, Some(U256::ZERO));
    assert!(
        program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .all(|statement| statement.operation != outer.id)
    );

    let memory = super::artifact(&cross(MASK160, MASK160, true));
    let stage = memory.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let text = copied_text(&high(stage, false));
    for (opcode, spelling) in [
        (0x51, "evm.mload("),
        (0x52, "evm.mstore("),
        (0x55, "evm.sstore("),
    ] {
        assert_eq!(text.matches(spelling).count(), 1, "{text}");
        let statement = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| operation(stage, statement) == opcode)
            .unwrap();
        assert!(
            program
                .render_index(&stage.ir)
                .render_statement(statement, |_| true)
                .unwrap()
                .contains(spelling)
        );
    }
}

#[test]
fn canonical_shared_bound_word_uses_the_same_mask_rhs_and_source_statement() {
    let artifact = artifact(&shared_mask(MASK160, MASK160));
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    let (procedure, body, target) = renderer
        .procedures()
        .find_map(|procedure| {
            let body = renderer
                .render_procedure_body(procedure, false, |_| true)
                .unwrap();
            let statement = program
                .blocks
                .iter()
                .flat_map(|block| &block.statements)
                .find(|statement| {
                    operation(stage, statement) == 0x16
                        && !statement.outputs.is_empty()
                        && body.iter().any(|row| row.entity == statement.id)
                })?;
            Some((procedure, body, statement))
        })
        .unwrap_or_else(|| {
            panic!(
                "shared bound AND missing; procedures={:?}\n{}",
                program.procedures,
                copied_text(&high(stage, false))
            )
        });
    assert!(
        program
            .invocations
            .iter()
            .filter(|call| call.procedure == procedure.id)
            .count()
            >= 2
    );
    let text = &body
        .iter()
        .find(|row| row.entity == target.id)
        .unwrap()
        .text;
    assert_eq!(
        text,
        &renderer
            .render_statement_with_dependencies(target, false, |_| true)
            .unwrap()
    );
    assert_eq!(text.matches(&format!("{MASK160:#x}")).count(), 1);
    assert_eq!(text.matches('&').count(), 1);
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = rows_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.entity == target.id && line.text == *text)
            );
            assert_eq!(
                copied_text(&lines).matches("evm.sload(").count(),
                1,
                "one read per displayed shared body"
            );
        }
    }
    let normal = high(stage, false);
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    let at = EntityRef {
        stage: stage.id,
        entity: target.id,
    };
    assert!(workspace.state.navigate(&artifact, at));
    let selected = frame(&ctx, &mut workspace, vec![]);
    assert!(
        selected
            .controls
            .iter()
            .any(|row| row["role"] == "decompile-code"
                && row["selected"] == true
                && row["label"].as_str().unwrap().ends_with(text))
    );
}
