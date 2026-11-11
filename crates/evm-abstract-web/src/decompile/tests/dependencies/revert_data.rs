//! General data is consumed from real publications, never inferred from text.
use super::*;
use crate::decompile::{index::StageIndex, structured::HighDisplayOptions};
use evm_abstract_protocol::decompile_model::{LiftedByteSegment, LiftedDataWord, ValidationStatus};

const PANIC: &str =
    include_str!("../../../../../evm-decompile/tests/fixtures/revert-data/panic-41-guard.hex");
const FAILED_ARGUMENTS: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/revert-data/custom-symbolic-failed-arm.hex"
);
const BOUND_ARGUMENTS: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/revert-data/custom-symbolic-bound-guard.hex"
);

#[test]
fn panic_and_symbolic_data_survive_publication_copy_source_links_and_display_options() {
    for (bytecode, panic, compact) in [
        (PANIC, true, true),
        (FAILED_ARGUMENTS, false, false),
        (BOUND_ARGUMENTS, false, true),
    ] {
        let artifact = artifact(bytecode.trim());
        let before = serde_json::to_vec(&artifact).unwrap();
        let wire = serde_json::to_value(&artifact).unwrap();
        let read: DecompileArtifact = serde_json::from_value(wire.clone()).unwrap();
        model::check_artifact(&read).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), wire);
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let [data] = program.revert_data.as_slice() else {
            panic!("{program:?}")
        };
        let symbolic = data
            .content
            .segments()
            .iter()
            .filter_map(|segment| match segment {
                LiftedByteSegment::Word {
                    value: LiftedDataWord::SourceValue(value),
                    ..
                } => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(symbolic.len(), if panic { 0 } else { 3 });
        let normal = high(stage, false);
        let text = copied_text(&normal);
        assert_eq!(text.contains("require("), compact, "{text}");
        if compact {
            assert!(
                !text.contains("evm.mstore(") && !text.contains("evm.mstore8("),
                "{text}"
            );
        }
        if panic {
            assert!(
                text.contains("Panic(0x41)") && text.contains("// 内存分配过大"),
                "{text}"
            );
        } else {
            assert!(
                text.contains("Revert(selector = 0xdeadbeef, words = ["),
                "{text}"
            );
            assert_eq!(text.matches("evm.calldataload(").count(), 4, "{text}");
            if compact {
                assert!(text.rfind("evm.calldataload(").unwrap() < text.find("require(").unwrap());
                let source_loads = stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .filter(|operation| operation.opcode == 0x35)
                    .collect::<Vec<_>>();
                let requirement = normal
                    .iter()
                    .find(|line| line.text.starts_with("require("))
                    .unwrap();
                for operation in source_loads.iter().take(3) {
                    assert!(
                        requirement
                            .text
                            .contains(&format!("v{}", operation.outputs[0].0)),
                        "{text}"
                    );
                }
            } else {
                assert!(text.contains("evm.revert(Revert("), "{text}");
                assert_eq!(text.matches("evm.mstore(").count(), 4, "{text}");
                assert_eq!(
                    normal
                        .iter()
                        .filter(|line| line.indent == 1 && line.text.contains("evm.calldataload("))
                        .count(),
                    3,
                    "{text}"
                );
            }
        }
        for dependencies in [false, true] {
            for annotations in [false, true] {
                let lines = guards::rows_with(
                    stage,
                    HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    },
                );
                assert_eq!(
                    lines
                        .iter()
                        .filter(|line| line.text.starts_with("require("))
                        .count(),
                    usize::from(compact)
                );
                assert!(copied_text(&lines).contains(if panic {
                    "Panic(0x41)"
                } else {
                    "Revert(selector = 0xdeadbeef, words = ["
                }));
            }
        }
        let raw = copied_text(&structured::lines(stage));
        assert!(
            raw.contains("evm.mstore(") && raw.contains("evm.revert("),
            "{raw}"
        );
        assert!(
            !raw.contains("Panic(") && !raw.contains("Revert(selector ="),
            "{raw}"
        );
        let index = StageIndex::new(&stage.ir);
        assert_eq!(index.revert_data(&stage.ir, data.id), Some(data));
        let (ctx, mut workspace) = setup(artifact.clone());
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
        let copied = click(&ctx, &mut workspace, "Copy structured text").copied;
        assert_eq!(copied.as_slice(), std::slice::from_ref(&text));
        let at = EntityRef {
            stage: stage.id,
            entity: data.id,
        };
        assert!(workspace.state.index.contains(at));
        assert!(!artifact.origin(at).unwrap().source.is_empty());
        let details = workspace.state.details(&artifact, at);
        assert!(details.lines[0].starts_with("Recovered REVERT data"));
        for reference in data.source_references() {
            let at = EntityRef {
                stage: stage.id,
                entity: reference,
            };
            assert!(
                workspace.state.index.contains(at),
                "missing data reference {at:?}"
            );
            assert!(
                details.references.contains(&at),
                "data reference not navigable: {at:?}"
            );
        }
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
    }
}

#[test]
fn invalid_data_sources_keep_raw_failure_rows_even_on_a_failed_snapshot() {
    let base = artifact(PANIC.trim());
    for status in [ValidationStatus::Passed, ValidationStatus::Failed] {
        let mut changed = base.clone();
        let current = changed.current_stage.unwrap();
        let stage = changed
            .stages
            .iter_mut()
            .find(|stage| stage.id == current)
            .unwrap();
        let data = &mut stage.ir.lifted.as_mut().unwrap().revert_data[0];
        data.operation = data.id;
        stage.validation.status = status;
        let text = copied_text(&high(stage, false));
        assert!(
            text.contains("if (")
                && text.contains("evm.mstore(")
                && text.contains("evm.revert(offset ="),
            "{text}"
        );
        assert!(
            !text.contains("require(") && !text.contains("Panic("),
            "{text}"
        );
        assert!(model::check_artifact(&changed).is_err());
    }
}

#[test]
fn zero_budget_data_inspection_keeps_source_links_without_a_format_claim() {
    let artifact = artifact(PANIC.trim());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let data = &program.revert_data[0];
    let index = StageIndex::new(&stage.ir);
    let renderer = program.render_base_index_with_budget(&stage.ir, |_| true);
    let work = crate::decompile::lifted::work::Work::new(0);
    let details =
        crate::decompile::lifted::details(stage, &index, data.id, &renderer, &work).unwrap();
    assert!(
        details
            .lines
            .iter()
            .any(|line| line.contains("work limit reached"))
    );
    assert!(
        !details
            .lines
            .iter()
            .any(|line| line.starts_with("Rendered data:"))
    );
    for entity in [data.block, data.operation] {
        assert!(details.references.contains(&EntityRef {
            stage: stage.id,
            entity
        }));
    }
}
