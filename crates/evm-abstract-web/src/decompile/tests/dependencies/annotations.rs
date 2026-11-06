//! Structure visibility is independent of state dependencies and semantic rows.
use super::*;
use crate::decompile::{index::StageIndex, structured::HighDisplayOptions};

fn high_with(stage: &Stage, display: HighDisplayOptions) -> Vec<structured::Line> {
    structured::lifted_lines_indexed(stage, &StageIndex::new(&stage.ir), display)
}

// Expected removable rows come from typed AST/IR witnesses. Fault comments and
// executable rows are deliberately absent from this set.
fn annotations(stage: &Stage) -> BTreeSet<(EntityId, String)> {
    let mut result = BTreeSet::new();
    let program = stage.ir.lifted.as_ref().unwrap();
    for block in &stage.ir.blocks {
        let owner = program.block(block.id).map_or(block.id, |block| block.id);
        result.insert((
            owner,
            format!(
                "// b{}{}",
                block.id.0,
                block.source.map_or_else(String::new, |source| format!(
                    " · source 0x{:04x}",
                    source.pc
                ))
            ),
        ));
    }
    structured::visit(
        &stage.ir.structured.as_ref().unwrap().root,
        &mut |node| match &node.kind {
            StructuredKind::Sequence(children) => {
                result.insert((
                    node.id,
                    format!("// sequence ast{} · {} children", node.id.0, children.len()),
                ));
            }
            StructuredKind::Dispatcher { cases, .. } => {
                for transition in cases.iter().flat_map(|case| &case.transitions) {
                    result.insert((transition.id, format!("// on e{}", transition.edge.0)));
                }
            }
            _ => {}
        },
    );
    result
}

#[test]
fn all_option_combinations_keep_exact_semantic_rows_effect_order_and_empty_else_elimination() {
    for bytecode in [
        EFFECTS_BRANCH,
        LOOP,
        DISPATCHER,
        "5f35600a57602a5f55005b5f5ffd",
        "5f35600a576001600d565b60025b5f5500",
    ] {
        let artifact = artifact(bytecode);
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let raw = rows(&structured::lines(stage));
        let removable = annotations(stage);
        for dependencies in [false, true] {
            let full = high_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: true,
                },
            );
            let clean = high(stage, dependencies);
            let expected: Vec<_> = rows(&full)
                .into_iter()
                .filter(|(entity, _, text)| !removable.contains(&(*entity, text.clone())))
                .collect();
            assert_eq!(rows(&clean), expected, "{bytecode}");
            assert!(full.len() > clean.len(), "exercise actual structural rows");
            if bytecode == DISPATCHER {
                assert!(full.iter().any(|line| line.text.starts_with("// on e")));
                assert!(
                    clean
                        .iter()
                        .any(|line| line.text.contains("dispatch_state_"))
                );
                assert!(
                    clean
                        .iter()
                        .any(|line| line.text.contains("continue 'dispatch"))
                );
            }
            if bytecode == "5f35600a57602a5f55005b5f5ffd" {
                assert_eq!(
                    clean.iter().filter(|line| line.text == "} else {").count(),
                    0,
                    "a certified guard stays flattened under every metadata option"
                );
                assert_eq!(
                    clean
                        .iter()
                        .filter(|line| line.text.starts_with("require("))
                        .count(),
                    1
                );
                assert!(!clean.iter().any(|line| line.text.contains("revert(")));
            }
        }
        assert_eq!(rows(&structured::lines(stage)), raw);
        assert_eq!(artifact, original);
    }
}

#[test]
fn default_structure_visibility_preserves_fault_comments_partial_and_invalid_boundaries() {
    for (bytecode, required) in [
        ("602a5f5255", "not completed"),
        ("5f3560010156", "unresolved_target (partial)"),
        ("5f3560ff57", "exceptional_invalid_target"),
    ] {
        let artifact = artifact(bytecode);
        let stage = artifact.current().unwrap();
        for dependencies in [false, true] {
            for structure in [false, true] {
                let lines = high_with(
                    stage,
                    HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: structure,
                    },
                );
                assert!(lines.iter().any(|line| line.text.contains(required)));
                if bytecode == "602a5f5255" {
                    let fault = lines
                        .iter()
                        .find(|line| line.text.contains(required))
                        .unwrap();
                    assert!(fault.text.starts_with("// fault"));
                    assert!(fault.text.contains("SSTORE"));
                    assert!(
                        stage
                            .ir
                            .lifted
                            .as_ref()
                            .unwrap()
                            .blocks
                            .iter()
                            .any(|block| {
                                block
                                    .statements
                                    .iter()
                                    .any(|statement| statement.id == fault.entity)
                            })
                    );
                }
            }
        }
    }
}

#[test]
fn actual_structure_toggle_copy_stages_routes_and_publications_preserve_independent_preferences() {
    let artifact = artifact(EFFECTS_BRANCH);
    let stage = artifact.current().unwrap();
    let normal = high(stage, false);
    let raw = structured::lines(stage);
    let annotated = high_with(
        stage,
        HighDisplayOptions {
            show_structure_annotations: true,
            ..HighDisplayOptions::default()
        },
    );
    let full = high_with(
        stage,
        HighDisplayOptions {
            show_state_dependencies: true,
            show_structure_annotations: true,
        },
    );
    let (ctx, mut workspace) = setup(artifact.clone());
    let stages = (workspace.state.before, workspace.state.current);
    let initial = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        control(&initial, "Show structure annotations")["role"],
        "toggle"
    );
    assert_eq!(
        control(&initial, "Show structure annotations")["selected"],
        false
    );
    assert_eq!(
        control(&initial, "Show state dependencies")["selected"],
        false
    );
    assert_painted(&initial, &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );

    // The release frame must already paint the new option, before another frame
    // has a chance to rebuild a stale cache.
    let changed = click(&ctx, &mut workspace, "Show structure annotations");
    assert_painted(&changed, &annotated);
    assert_eq!(
        control(&changed, "Show state dependencies")["selected"],
        false
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&annotated)]
    );
    assert_painted(
        &click(&ctx, &mut workspace, "Show state dependencies"),
        &full,
    );

    let raw_frame = click(&ctx, &mut workspace, "Structured code");
    for label in ["Show structure annotations", "Show state dependencies"] {
        assert!(
            !raw_frame
                .controls
                .iter()
                .any(|control| control["label"] == label)
        );
    }
    assert_painted(&raw_frame, &raw);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&raw)]
    );
    click(&ctx, &mut workspace, "IR");
    assert_painted(
        &click(&ctx, &mut workspace, "Simplified structured code"),
        &full,
    );
    assert_painted(
        &click(&ctx, &mut workspace, "Show structure annotations"),
        &high(stage, true),
    );
    let hidden = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        control(&hidden, "Show structure annotations")["selected"],
        false
    );
    assert_eq!(
        control(&hidden, "Show state dependencies")["selected"],
        true
    );
    assert_eq!((workspace.state.before, workspace.state.current), stages);

    let raw_stage = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap();
    workspace.state.set_stage(&artifact, raw_stage.id);
    let unavailable = frame(&ctx, &mut workspace, vec![]);
    assert!(
        !unavailable
            .controls
            .iter()
            .any(|control| control["label"] == "Show structure annotations")
    );
    workspace.state.set_stage(&artifact, stage.id);
    assert_painted(
        &click(&ctx, &mut workspace, "Show structure annotations"),
        &full,
    );
    deliver(&mut workspace, artifact.clone(), 11);
    let installed = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        control(&installed, "Show structure annotations")["selected"],
        true
    );
    assert_eq!(
        control(&installed, "Show state dependencies")["selected"],
        true
    );
    assert_painted(&installed, &full);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&full)]
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
}

#[test]
fn hidden_block_metadata_falls_back_to_real_ir_and_explicit_routes_keep_working() {
    let artifact = artifact(EFFECTS_BRANCH);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let clean = high(stage, false);
    let annotated = high_with(
        stage,
        HighDisplayOptions {
            show_structure_annotations: true,
            ..HighDisplayOptions::default()
        },
    );
    let block = program
        .blocks
        .iter()
        .find(|block| {
            !clean.iter().any(|line| line.entity == block.id)
                && annotated.iter().any(|line| line.entity == block.id)
        })
        .expect("exercise a block owned only by its structure header");
    let at = EntityRef {
        stage: stage.id,
        entity: block.id,
    };
    let (ctx, mut workspace) = setup(artifact.clone());
    assert!(workspace.state.navigate(&artifact, at));
    let navigated = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "IR");
    assert!(
        navigated
            .controls
            .iter()
            .any(|control| { control["role"] == "decompile-ir" && control["selected"] == true })
    );
    assert!(!workspace.state.entity_navigation);
    assert_painted(
        &click(&ctx, &mut workspace, "Simplified structured code"),
        &clean,
    );
    click(&ctx, &mut workspace, "Show structure annotations");
    assert!(workspace.state.navigate(&artifact, at));
    let navigated = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(navigated.controls.iter().any(|control| {
        control["role"] == "decompile-code"
            && control["selected"] == true
            && control["label"]
                .as_str()
                .unwrap()
                .contains(&format!("// b{}", block.block.0))
    }));
    assert_painted(
        &click(&ctx, &mut workspace, "Show structure annotations"),
        &clean,
    );

    let mut sequence = None;
    structured::visit(&stage.ir.structured.as_ref().unwrap().root, &mut |node| {
        if matches!(node.kind, StructuredKind::Sequence(_)) {
            sequence = Some(node.id);
        }
    });
    let at = EntityRef {
        stage: stage.id,
        entity: sequence.expect("real sequence"),
    };
    assert!(workspace.state.navigate(&artifact, at));
    let navigated = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Structured code");
    assert!(navigated.controls.iter().any(|control| {
        control["role"] == "decompile-code"
            && control["selected"] == true
            && control["label"]
                .as_str()
                .unwrap()
                .contains(&format!("sequence ast{}", at.entity.0))
    }));
    assert_eq!(workspace.state.selected.as_ref().unwrap().at, at);
    assert_painted(
        &click(&ctx, &mut workspace, "Simplified structured code"),
        &clean,
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
}
