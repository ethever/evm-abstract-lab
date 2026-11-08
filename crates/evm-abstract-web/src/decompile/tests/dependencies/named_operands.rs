//! Native published-artifact tests cover code rows, Inspector and real clipboard.
use super::{
    artifact, assert_painted, click, copied_text, frame, high, labels, setup, without_annotations,
};
use crate::decompile::{index::StageIndex, structured};
use evm_abstract_protocol::decompile_model::{
    EntityRef, ExitKind, LiftedExpressionKind, StageKind, Terminator, U256,
};

const DISTINCT_CALL: &str = "6077606660556044603360226011f15000";
const NAMED_CALL: &str = "evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);";
// Each pure result crosses a real JUMP before its next use, requiring a retained
// SSA binding. The memory reads and nonempty REVERT remain detailed primitives.
const RETAINED_OPERATORS: &str = concat!(
    "600360025f3760025f5101600e565b60031b6015565b60041c601c565b19",
    "6021565b6002900a6029565b5f5560035ffd",
);

#[test]
fn published_call_keeps_named_values_in_code_inspector_and_clipboard_only() {
    let artifact = artifact(DISTINCT_CALL);
    let original = artifact.clone();
    let encoded = serde_json::to_vec(&artifact).unwrap();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let operation = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| operation.opcode == 0xf1)
        .unwrap();
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == operation.id)
        .unwrap();
    let at = EntityRef {
        stage: stage.id,
        entity: statement.id,
    };
    let expected_values = [0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77].map(U256::from);
    let witnessed: Vec<_> = operation
        .inputs
        .iter()
        .map(|id| stage.ir.value(*id).unwrap().fact.singleton().unwrap())
        .collect();
    assert_eq!(witnessed, expected_values);
    let raw_arguments = operation
        .inputs
        .iter()
        .map(|id| format!("v{}", id.0))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(operation.outputs.len(), 1);
    let positional = format!("v{} = evm.call({raw_arguments});", operation.outputs[0].0);
    let raw_rows = structured::lines(stage);
    assert!(
        raw_rows
            .iter()
            .any(|line| line.entity == operation.id && line.text.starts_with(&positional))
    );
    assert!(!copied_text(&raw_rows).contains("gas = "));

    for dependencies in [false, true] {
        let lines = high(stage, dependencies);
        let call = lines
            .iter()
            .find(|line| line.entity == statement.id)
            .unwrap();
        assert!(call.text.starts_with(NAMED_CALL), "{}", call.text);
        assert_eq!(call.text.contains(" // memory"), dependencies);
        assert_eq!(call.text.contains(" // effect"), dependencies);
        assert!(lines.iter().all(|line| !line.text.contains('\n')));
    }
    let (ctx, mut workspace) = setup(artifact.clone());
    let initial = frame(&ctx, &mut workspace, vec![]);
    assert_painted(&initial, &high(stage, false));
    assert!(
        labels(&initial, "decompile-code")
            .iter()
            .any(|line| line.ends_with(NAMED_CALL))
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&high(stage, false))]
    );

    click(&ctx, &mut workspace, "Transform inspector");
    workspace.state.select(&artifact, at);
    let inspector = labels(
        &frame(&ctx, &mut workspace, vec![]),
        "decompile-entity-detail",
    );
    assert!(
        inspector.iter().any(|line| line.contains(NAMED_CALL)),
        "{inspector:?}"
    );
    click(&ctx, &mut workspace, "Show state dependencies");
    let debug_copy = click(&ctx, &mut workspace, "Copy structured text").copied;
    assert_eq!(debug_copy, [copied_text(&high(stage, true))]);
    assert!(debug_copy[0].contains(NAMED_CALL));
    assert_eq!(
        labels(
            &frame(&ctx, &mut workspace, vec![]),
            "decompile-entity-detail"
        ),
        inspector
    );

    let raw_frame = click(&ctx, &mut workspace, "Structured code");
    assert!(
        labels(&raw_frame, "decompile-code")
            .iter()
            .any(|line| line.contains(&positional))
    );
    let raw_copy = click(&ctx, &mut workspace, "Copy structured text").copied;
    assert_eq!(raw_copy, [copied_text(&raw_rows)]);
    assert!(!raw_copy[0].contains("gas = "));
    let ir_frame = click(&ctx, &mut workspace, "IR");
    let ir_before = labels(&ir_frame, "decompile-ir");
    let operation_row = ir_before
        .iter()
        .find(|line| line.contains(&format!("o{}: ", operation.id.0)))
        .unwrap();
    assert!(operation_row.contains(&positional), "{operation_row}");
    assert!(!operation_row.contains("gas = "));
    click(&ctx, &mut workspace, "Simplified structured code");
    click(&ctx, &mut workspace, "Show state dependencies");
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&high(stage, false))]
    );
    assert_eq!(
        labels(&click(&ctx, &mut workspace, "IR"), "decompile-ir"),
        ir_before
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&original));
    assert_eq!(
        serde_json::to_vec(workspace.artifact.as_ref().unwrap()).unwrap(),
        encoded
    );
    assert_eq!(artifact, original);
}

#[test]
fn published_terminal_rows_follow_operand_policy_while_raw_exits_stay_positional() {
    for (bytecode, kind, expected) in [
        (
            "60226011f3",
            ExitKind::Return,
            "evm.return(offset = 0x11, size = 0x22);",
        ),
        (
            "60226011fd",
            ExitKind::Revert,
            "evm.revert(offset = 0x11, size = 0x22);",
        ),
        ("6011ff", ExitKind::SelfDestruct, "evm.selfdestruct(0x11);"),
        ("00", ExitKind::Stop, "evm.stop();"),
        ("601150", ExitKind::EndOfCode, "evm.stop(); // end of code"),
    ] {
        let artifact = artifact(bytecode);
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let block = stage.ir.blocks.iter().find(|block| matches!(block.terminator, Terminator::Exit { kind: actual, .. } if actual == kind)).unwrap();
        let semantic = program.block(block.id).unwrap();
        for dependencies in [false, true] {
            for annotations in [false, true] {
                let lines = structured::lifted_lines_indexed(
                    stage,
                    &StageIndex::new(&stage.ir),
                    structured::HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    },
                );
                let exits: Vec<_> = lines.iter().filter(|line| line.text == expected).collect();
                assert_eq!(exits.len(), 1, "{bytecode}: {}", copied_text(&lines));
                let owner = semantic
                    .statements
                    .iter()
                    .find(|statement| {
                        block.operations.iter().any(|operation| {
                            operation.id == statement.operation && operation.effects.terminate
                        })
                    })
                    .map_or(semantic.id, |statement| statement.id);
                assert_eq!(exits[0].entity, owner);
                assert_eq!(exits[0].indent, 0);
            }
        }
        // Derive only the raw SSA identities from the witness, never its roles.
        let Terminator::Exit { values, .. } = &block.terminator else {
            unreachable!()
        };
        let operands = values
            .iter()
            .map(|id| format!("v{}", id.0))
            .collect::<Vec<_>>()
            .join(", ");
        let raw_expected = match kind {
            ExitKind::Return => format!("evm.return({operands});"),
            ExitKind::Revert => format!("evm.revert({operands});"),
            ExitKind::SelfDestruct => format!("evm.selfdestruct({operands});"),
            ExitKind::Stop => "evm.stop();".into(),
            ExitKind::EndOfCode => "evm.stop(); // end of code".into(),
        };
        assert!(
            structured::lines(stage)
                .iter()
                .any(|line| line.text == raw_expected)
        );
        let prior = artifact
            .stages
            .iter()
            .find(|stage| stage.kind == StageKind::Structured)
            .unwrap();
        assert!(
            structured::lines(prior)
                .iter()
                .any(|line| line.text == raw_expected)
        );
        let mut witness = stage.ir.clone();
        witness.lifted = None;
        assert_eq!(witness, prior.ir);
        assert_eq!(artifact, original);
    }
}

#[test]
fn retained_operators_keep_output_names_source_references_and_detailed_memory_rows() {
    let artifact = artifact(RETAINED_OPERATORS);
    let encoded = serde_json::to_vec(&artifact).unwrap();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let retained = |opcode| {
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|operation| operation.opcode == opcode)
            .unwrap();
        let statement = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| statement.operation == operation.id)
            .unwrap();
        assert_eq!(statement.outputs, operation.outputs);
        (operation, statement)
    };
    let (copy_op, copy) = retained(0x37);
    let (load_op, load) = retained(0x51);
    let (add_op, add) = retained(0x01);
    let (shl_op, shl) = retained(0x1b);
    let (shr_op, shr) = retained(0x1c);
    let (not_op, not) = retained(0x19);
    let (exp_op, exp) = retained(0x0a);
    let (revert_op, revert) = retained(0xfd);
    for operation in [add_op, shl_op, shr_op, not_op, exp_op] {
        let block = stage
            .ir
            .blocks
            .iter()
            .find(|block| block.operations.contains(operation))
            .unwrap();
        assert!(stage.ir.edges.iter().any(|edge| {
            edge.from == block.id && edge.arguments.contains(&operation.outputs[0])
        }));
    }
    let value = |input| {
        let LiftedExpressionKind::Value(id) = program.expression(input).unwrap().kind else {
            panic!("the cross-block operand must retain a source word binding")
        };
        format!("v{}", id.0)
    };
    for (input, expected) in [
        (add.inputs[1], 2),
        (shl.inputs[0], 3),
        (shr.inputs[0], 4),
        (exp.inputs[1], 2),
    ] {
        assert_eq!(
            program.expression(input).unwrap().kind,
            LiftedExpressionKind::Word(U256::from(expected))
        );
    }
    let expected = [
        (
            copy_op,
            copy,
            "evm.calldatacopy(memory_offset = 0x0, calldata_offset = 0x2, size = 0x3);".to_owned(),
        ),
        (
            load_op,
            load,
            format!("v{} = evm.mload(offset = 0x0);", load.outputs[0].0),
        ),
        (
            add_op,
            add,
            format!("v{} = ({} + 0x2);", add.outputs[0].0, value(add.inputs[0])),
        ),
        (
            shl_op,
            shl,
            format!("v{} = ({} << 0x3);", shl.outputs[0].0, value(shl.inputs[1])),
        ),
        (
            shr_op,
            shr,
            format!("v{} = ({} >> 0x4);", shr.outputs[0].0, value(shr.inputs[1])),
        ),
        (
            not_op,
            not,
            format!("v{} = (~{});", not.outputs[0].0, value(not.inputs[0])),
        ),
        (
            exp_op,
            exp,
            format!("v{} = ({} ** 0x2);", exp.outputs[0].0, value(exp.inputs[0])),
        ),
        (
            revert_op,
            revert,
            "evm.revert(offset = 0x0, size = 0x3);".to_owned(),
        ),
    ];
    let normal = high(stage, false);
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = structured::lifted_lines_indexed(
                stage,
                &StageIndex::new(&stage.ir),
                structured::HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            for (_, statement, text) in &expected {
                let row = lines.iter().find(|row| row.entity == statement.id).unwrap();
                assert_eq!(without_annotations(&row.text), text);
            }
            for statement in [copy, load] {
                let row = lines.iter().find(|row| row.entity == statement.id).unwrap();
                assert_eq!(row.text.contains(" // memory"), dependencies);
            }
        }
    }
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    click(&ctx, &mut workspace, "Transform inspector");
    for (operation, statement, text) in &expected {
        let at = EntityRef {
            stage: stage.id,
            entity: statement.id,
        };
        let pc = operation.source.unwrap().pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, pc)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let selected = frame(&ctx, &mut workspace, vec![]);
        assert!(selected.controls.iter().any(|row| {
            row["role"] == "decompile-code"
                && row["selected"] == true
                && row["label"].as_str().unwrap().ends_with(text)
        }));
        let details = workspace.state.details(&artifact, at);
        assert!(
            details
                .lines
                .iter()
                .any(|line| { line.starts_with("Statement") && line.contains(text) })
        );
        for entity in std::iter::once(statement.operation)
            .chain(statement.inputs.iter().copied())
            .chain(statement.outputs.iter().copied())
        {
            assert!(details.references.contains(&EntityRef {
                stage: stage.id,
                entity,
            }));
        }
    }
    assert_eq!(
        serde_json::to_vec(workspace.artifact.as_ref().unwrap()).unwrap(),
        encoded
    );
}
