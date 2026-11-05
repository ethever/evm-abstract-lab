//! Native published-artifact tests cover code rows, Inspector and real clipboard.
use super::{artifact, assert_painted, click, copied_text, frame, high, labels, setup};
use crate::decompile::{index::StageIndex, structured};
use evm_abstract_protocol::decompile_model::{EntityRef, ExitKind, StageKind, Terminator, U256};

const DISTINCT_CALL: &str = "6077606660556044603360226011f15000";
const NAMED_CALL: &str = "call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);";

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
fn published_terminal_rows_use_named_semantic_values_but_raw_exits_stay_positional() {
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
        (
            "6011ff",
            ExitKind::SelfDestruct,
            "evm.selfdestruct(beneficiary = 0x11);",
        ),
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
                assert_eq!(exits[0].entity, semantic.id);
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
