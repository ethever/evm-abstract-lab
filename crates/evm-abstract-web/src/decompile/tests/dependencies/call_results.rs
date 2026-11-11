//! Real publications share one call-buffer display across code and Inspector.
use super::*;
use crate::decompile::index::StageIndex;
use evm_abstract_protocol::decompile_model::{
    LiftedByteSegment, LiftedByteSource, LiftedConstructionKind, ProcedurePreference,
    ReadabilityConfig, ValidationStatus,
};

mod scopes;

const PRECOPY: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/call-results/delegate-precopy-return.hex"
);
const REPEATED: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/call-results/delegate-repeated-sizes.hex"
);
const NESTED: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/call-results/delegate-nested-buffers.hex"
);

fn inline(bytecode: &str) -> DecompileArtifact {
    artifact_with_readability(
        bytecode.trim(),
        ReadabilityConfig {
            procedures: ProcedurePreference::Inline,
            ..Default::default()
        },
    )
}

#[test]
fn call_buffers_keep_unconditional_copy_and_exact_raw_witnesses_in_every_display() {
    for (bytecode, count, reads) in [(PRECOPY, 1, 2), (REPEATED, 1, 3), (NESTED, 2, 4)] {
        let artifact = inline(bytecode);
        let before = serde_json::to_vec(&artifact).unwrap();
        let stage = artifact.current().unwrap();
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.validation.status == ValidationStatus::Passed)
        );
        let program = stage.ir.lifted.as_ref().unwrap();
        let mut calls = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter(|op| op.opcode == 0xf4)
            .collect::<Vec<_>>();
        calls.sort_by_key(|op| op.source.as_ref().unwrap().pc);
        assert_eq!(calls.len(), count);
        let normal = high(stage, false);
        let text = copied_text(&normal);
        assert_eq!(text.matches("require(").count(), count, "{text}");
        assert_eq!(text.matches("evm.returndatacopy(").count(), count, "{text}");
        assert!(
            text.contains("evm.return(") && !text.contains("evm.revert("),
            "{text}"
        );
        assert!(
            !text.contains("returndata(op") && !text.contains("evm.returndatasize("),
            "{text}"
        );
        let tokens: BTreeSet<_> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
        for reader in stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter(|op| op.opcode == 0x3d)
        {
            for output in &reader.outputs {
                assert!(
                    !tokens.contains(format!("v{}", output.0).as_str()),
                    "{text}"
                );
            }
        }
        let mut previous = 0;
        for (index, call) in calls.iter().enumerate() {
            let ordinal = index + 1;
            let declaration =
                format!("(success{ordinal}, returndata{ordinal}) = evm.delegatecall(");
            assert_eq!(text.matches(&declaration).count(), 1, "{text}");
            let at_call = text.find(&declaration).unwrap();
            let at_copy = text[at_call..].find("evm.returndatacopy(").unwrap() + at_call;
            let at_guard = text[at_copy..].find("require(").unwrap() + at_copy;
            assert!(
                previous <= at_call && at_call < at_copy && at_copy < at_guard,
                "{text}"
            );
            previous = at_guard;
            for marker in ["evm.returndatacopy(", "require("] {
                let row = normal
                    .iter()
                    .find(|line| {
                        line.text.starts_with(marker)
                            && line.text.contains(&format!("returndata{ordinal}"))
                    })
                    .unwrap();
                assert_eq!(row.indent, 0, "{text}");
            }
            let data = program.revert_data.iter().find(|data| data.content.segments().iter().any(|segment|
                matches!(segment, LiftedByteSegment::Slice { source: LiftedByteSource::ReturnData(source), .. } if *source == call.id)
            )).unwrap();
            let construction = program
                .constructions
                .iter()
                .find(|plan| plan.revert == data.id)
                .unwrap();
            assert_eq!(construction.kind, LiftedConstructionKind::PreparedConsumer);
            let block = program
                .blocks
                .iter()
                .find(|block| block.block == construction.block)
                .unwrap();
            let consumed = &block.statements[construction.start..construction.end];
            let opcodes = consumed
                .iter()
                .map(|statement| {
                    stage
                        .ir
                        .blocks
                        .iter()
                        .flat_map(|block| &block.operations)
                        .find(|op| op.id == statement.operation)
                        .unwrap()
                        .opcode
                })
                .collect::<Vec<_>>();
            assert!(opcodes.contains(&0x3d) && !opcodes.contains(&0x3e));
            assert_eq!(opcodes.last(), Some(&0xfd));
        }
        for dependencies in [false, true] {
            for annotations in [false, true] {
                let lines = guards::rows_with(
                    stage,
                    structured::HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    },
                );
                let semantic = lines
                    .iter()
                    .filter(|line| {
                        !line.text.starts_with("//") && !line.text.contains(" token on e")
                    })
                    .map(|line| (line.entity, line.indent, without_annotations(&line.text)))
                    .collect::<Vec<_>>();
                let expected = normal
                    .iter()
                    .map(|line| (line.entity, line.indent, line.text.as_str()))
                    .collect::<Vec<_>>();
                assert_eq!(semantic, expected);
            }
        }
        let raw = copied_text(&structured::lines(stage));
        for (name, expected) in [
            ("evm.delegatecall(", count),
            ("evm.returndatacopy(", count),
            ("evm.returndatasize(", reads),
            ("evm.revert(", count),
            ("evm.return(", 1),
        ] {
            assert_eq!(raw.matches(name).count(), expected, "{raw}");
        }
        assert!(
            !raw.contains("returndata1.size")
                && !raw.contains("success1")
                && !raw.contains("require(")
        );
        let read: DecompileArtifact = serde_json::from_slice(&before).unwrap();
        model::check_artifact(&read).unwrap();
        assert_eq!(serde_json::to_vec(&read).unwrap(), before);
    }
}

#[test]
fn call_source_roles_survive_inspector_clipboard_and_precise_source_navigation() {
    for bytecode in [PRECOPY, REPEATED, NESTED] {
        let artifact = inline(bytecode);
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let renderer = program.render_base_index_with_budget(&stage.ir, |_| true);
        let index = StageIndex::new(&stage.ir);
        let normal = high(stage, false);
        let text = copied_text(&normal);
        let (ctx, mut workspace) = setup(artifact.clone());
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
        let copied = click(&ctx, &mut workspace, "Copy structured text").copied;
        assert_eq!(copied.as_slice(), std::slice::from_ref(&text));
        for op in stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter(|op| matches!(op.opcode, 0xf4 | 0x3d | 0x3e))
        {
            let statement = program
                .blocks
                .iter()
                .flat_map(|block| &block.statements)
                .find(|statement| statement.operation == op.id)
                .unwrap();
            for entity in [statement.id, op.id] {
                let at = EntityRef {
                    stage: stage.id,
                    entity,
                };
                let details = workspace.state.details(&artifact, at);
                if let Some(call) = renderer.call_result(op.id) {
                    assert!(details.lines.iter().any(|line| line.starts_with(&format!("Call result #{}", call.ordinal))), "{:?}", details.lines);
                    for source in call.source_references() {
                        assert!(details.references.contains(&EntityRef {
                            stage: stage.id,
                            entity: source
                        }));
                    }
                } else {
                    let call = renderer.returndata_source(op.id).unwrap();
                    assert!(
                        details
                            .lines
                            .iter()
                            .any(|line| line == &format!("Returndata source: o{}", call.0)),
                        "{:?}",
                        details.lines
                    );
                    assert!(details.references.contains(&EntityRef {
                        stage: stage.id,
                        entity: call
                    }));
                }
                let pc = op.source.as_ref().unwrap().pc;
                assert!(
                    artifact
                        .origin(at)
                        .unwrap()
                        .source
                        .iter()
                        .any(|span| span.pc <= pc && pc < span.end_pc)
                );
                assert!(
                    workspace
                        .state
                        .source_descendants(&artifact, pc)
                        .contains(&at)
                );
                assert!(index.operation(&stage.ir, op.id).is_some());
            }
        }
    }
}
