//! Certified sharing crosses publication, cached code, clipboard and navigation.
use super::*;
use crate::decompile::model::EntityKind;
use evm_abstract_protocol::decompile_model::{
    LiftedExpressionKind, SharedProcedureKind, U256, ValidationStatus,
};
use std::collections::BTreeMap;

fn artifact(bytecode: &str) -> DecompileArtifact {
    super::artifact_with_readability(
        bytecode,
        evm_abstract_protocol::decompile_model::ReadabilityConfig {
            procedures: evm_abstract_protocol::decompile_model::ProcedurePreference::Shared,
            ..Default::default()
        },
    )
}

// Two different caller words enter the same two-block helper. Each call writes
// memory/storage and emits LOG0, then returns two words to a distinct caller.
const SHARED: &str = concat!(
    "60075f35601e565b6001556002556016602035601e565b60035560045500",
    "5b80604052805f5560206040a0600554600101600555600654600755603d565b806001019156",
);
const SHARED_LOOP: &str = include_str!("../../../../tests/fixtures/decompile/shared-loop.hex");
const ARITHMETIC: &str = include_str!(
    "../../../../../evm-decompile/tests/fixtures/shared-procedures/arithmetic-template-inputs.hex"
);
// The helper's false arm has only an empty block and m/q transfers. Two callers
// retain the live word result, and the true arm has observable memory/log/store.
const SHARED_TOKEN_ELSE: &str = concat!(
    "61000a60003561001d565b60015561001860203561001d565b60025500",
    "5b80604052806000558061002f57610041565b8060605260206040a0",
    "80600355806004555b6001019056",
);
const SHARED_EMPTY_THEN: &str =
    include_str!("../../../../tests/fixtures/decompile/shared-empty-then.hex");

// Two return-address contexts retain a failed arm that reads/writes memory and
// reverts. Only the locally returning successful work may become a procedure.
const SHARED_FAILURE: &str = concat!(
    "6100095f3561001c565b60015561001760203561001c565b600255005b8061004257",
    "80604052805f5560206040a06005546001016005556006546007556001019056",
    "5b604051602a905260205ffd",
);

#[test]
fn shared_success_body_keeps_certified_checks_in_their_actual_caller_scopes() {
    let mut runtime = guards::Runtime::default();
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
    runtime.emit(&[0x80]);
    runtime.jump("success", true);
    runtime.emit(&[0x5f, 0x5f, 0xfd]);
    runtime.label("success");
    // Observable work gives the repeated checked helper a real sharing gain.
    runtime.emit(&[
        0x80, 0x60, 0x40, 0x52, 0x80, 0x5f, 0x55, 0x60, 0x20, 0x60, 0x40, 0xa0,
    ]);
    runtime.emit(&[0x60, 0x05, 0x54, 0x60, 0x01, 0x01, 0x60, 0x05, 0x55]);
    runtime.emit(&[
        0x60, 0x06, 0x54, 0x60, 0x07, 0x55, 0x60, 0x01, 0x01, 0x90, 0x56,
    ]);
    let artifact = artifact(&runtime.finish());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    let [procedure] = program.procedures.as_slice() else {
        panic!("the repeated successful helper must share")
    };
    let canonical = renderer
        .render_procedure_body(procedure, false, |_| true)
        .unwrap();
    assert!(
        !canonical
            .iter()
            .any(|row| row.text.starts_with("require(") || row.text.contains("evm.revert("))
    );
    assert!(
        program
            .invocations
            .iter()
            .filter(|call| call.procedure == procedure.id)
            .count()
            >= 2
    );
    let normal = high(stage, false);
    assert_eq!(
        normal
            .iter()
            .filter(|line| line.text.starts_with("require(") && line.indent == 0)
            .count(),
        2
    );
    assert!(!copied_text(&normal).contains("evm.revert("));
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    for control in program
        .controls
        .iter()
        .filter(|control| control.guard.is_some())
    {
        let required = renderer.render_guard(control, |_| true).unwrap().unwrap();
        let at = EntityRef {
            stage: stage.id,
            entity: control.id,
        };
        assert!(workspace.state.navigate(&artifact, at));
        let visible = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert!(
            visible
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
        let details = workspace.state.details(&artifact, at);
        assert!(
            details
                .lines
                .iter()
                .any(|line| line.starts_with("Certified empty-REVERT guard")
                    && line.contains(&required)),
            "{:?}",
            details.lines
        );
        for entity in [
            control.condition,
            control.node,
            control.guard.as_ref().unwrap().failure_node,
        ] {
            assert!(details.references.contains(&EntityRef {
                stage: stage.id,
                entity
            }));
        }
        assert!(workspace.state.navigate(
            &artifact,
            EntityRef {
                stage: stage.id,
                entity: control.node
            }
        ));
        frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Structured code");
    }
}

#[test]
fn shared_success_keeps_failure_construction_and_terminal_in_each_caller() {
    let artifact = artifact(SHARED_FAILURE);
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    assert!(!program.procedures.is_empty());
    let failure_rows: Vec<_> = program
        .blocks
        .iter()
        .filter(|block| {
            stage
                .ir
                .block(block.block)
                .unwrap()
                .operations
                .iter()
                .any(|operation| operation.opcode == 0xfd)
        })
        .flat_map(|block| &block.statements)
        .filter(|statement| {
            stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|operation| {
                    operation.id == statement.operation
                        && matches!(operation.opcode, 0x51 | 0x52 | 0xfd)
                })
        })
        .map(|statement| statement.id)
        .collect();
    assert_eq!(
        failure_rows.len(),
        6,
        "both failure contexts retain MLOAD, MSTORE and REVERT"
    );
    let raw = rows(&structured::lines(stage));
    for dependencies in [false, true] {
        for procedure in &program.procedures {
            let body = renderer
                .render_procedure_body(procedure, dependencies, |_| true)
                .unwrap();
            assert!(body.iter().all(|row| !failure_rows.contains(&row.entity)));
            assert!(body.iter().all(|row| !row.text.contains("evm.revert(")));
        }
        for annotations in [false, true] {
            let lines = with_display(
                stage,
                structured::HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            for entity in &failure_rows {
                assert_eq!(
                    lines.iter().filter(|line| line.entity == *entity).count(),
                    1
                );
            }
            let text = copied_text(&lines);
            assert_eq!(text.matches("evm.revert(offset =").count(), 2);
            assert!(!text.contains("require("));
        }
    }
    assert_semantic_rows_survive_toggles(stage);
    let normal = high(stage, false);
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    for entity in failure_rows {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        let source = artifact.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let selected = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert!(
            selected
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
    }
    assert_eq!(rows(&structured::lines(stage)), raw);
    assert_eq!(artifact, original);
}

fn with_display(stage: &Stage, display: structured::HighDisplayOptions) -> Vec<structured::Line> {
    structured::lifted_lines_indexed(
        stage,
        &crate::decompile::index::StageIndex::new(&stage.ir),
        display,
    )
}

#[test]
fn cached_shared_else_uses_one_semantic_layout_in_all_display_modes() {
    let artifact = artifact(SHARED_TOKEN_ELSE);
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(program.procedures.len(), 1);
    assert_eq!(program.invocations.len(), 2);
    let renderer = program.render_index(&stage.ir);
    let procedure = &program.procedures[0];
    let branch = renderer.node(procedure.body).unwrap();
    let control = program
        .controls
        .iter()
        .find(|control| control.node == branch.id)
        .unwrap();
    let StructuredKind::If { else_branch, .. } = &branch.kind else {
        panic!("the certified shared body must contain the conditional itself")
    };
    let mut tokens = Vec::new();
    let mut context = BTreeSet::new();
    structured::visit(else_branch, &mut |node| match &node.kind {
        StructuredKind::Transfer { edge } => {
            assert!(renderer.transfer(*edge).unwrap().arguments.is_empty());
            let edge = stage.ir.edge(*edge).unwrap();
            let target = stage.ir.block(edge.to).unwrap();
            tokens.extend([
                (
                    edge.id,
                    2,
                    format!(
                        "m{} = m{}; // memory token on e{}",
                        target.memory_parameter.0, edge.memory.0, edge.id.0
                    ),
                ),
                (
                    edge.id,
                    2,
                    format!(
                        "q{} = q{}; // effect token on e{}",
                        target.effect_parameter.0, edge.effect.0, edge.id.0
                    ),
                ),
            ]);
        }
        StructuredKind::Sequence(_) | StructuredKind::Block { .. } => {
            context.insert(node.id);
        }
        _ => panic!("false arm must contain only context and state transfers"),
    });
    assert!(!tokens.is_empty());
    assert!(!context.is_empty(), "exercise annotation-only block rows");
    let index = crate::decompile::index::StageIndex::new(&stage.ir);
    let raw = rows(&structured::lines(stage));
    for (dependencies, annotations) in [
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ] {
        let lines = structured::lifted_lines_indexed(
            stage,
            &index,
            structured::HighDisplayOptions {
                show_state_dependencies: dependencies,
                show_structure_annotations: annotations,
            },
        );
        let rendered = rows(&lines);
        assert_eq!(
            rendered
                .iter()
                .filter(|(entity, depth, text)| *entity == control.id
                    && *depth == 1
                    && text == "} else {")
                .count(),
            0,
            "{rendered:#?}"
        );
        assert!(rendered.iter().any(|(entity, depth, text)| {
            *entity == control.id && *depth == 1 && text.starts_with("if (")
        }));
        assert!(rendered.contains(&(control.id, 1, "}".into())));
        for token in &tokens {
            assert!(!rendered.contains(token), "{rendered:#?}");
        }
        assert!(
            !rendered
                .iter()
                .any(|(entity, _, _)| context.contains(entity))
        );
        let copied = copied_text(&lines);
        assert_eq!(copied.matches("} else {").count(), 0);
        assert_eq!(copied.matches("evm.log0(").count(), 1);
        assert_eq!(copied.matches("evm.sstore(slot = 0x3,").count(), 1);
        assert_eq!(copied.matches("evm.sstore(slot = 0x4,").count(), 1);
        assert_eq!(rows(&structured::lines(stage)), raw);
    }
    assert_eq!(artifact, original);
}

#[test]
fn shared_empty_then_layout_survives_expansion_copy_and_condition_navigation() {
    let artifact = artifact(SHARED_EMPTY_THEN.trim());
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = crate::decompile::index::StageIndex::new(&stage.ir);
    let renderer = program.render_index(&stage.ir);
    let procedure = program
        .procedures
        .iter()
        .find(|procedure| {
            renderer
                .node(procedure.body)
                .is_some_and(|node| matches!(node.kind, StructuredKind::If { .. }))
        })
        .expect("the repeated conditional itself must share");
    let canonical = program
        .controls
        .iter()
        .find(|control| control.node == procedure.body)
        .unwrap();
    let call = program
        .invocations
        .iter()
        .find(|call| call.procedure == procedure.id && call.source != procedure.body)
        .unwrap();
    let actual = program
        .controls
        .iter()
        .find(|control| control.node == call.source)
        .unwrap();
    let expanded_calls = BTreeSet::from([call.id]);
    let raw = rows(&structured::lines(stage));
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let display = structured::HighDisplayOptions {
                show_state_dependencies: dependencies,
                show_structure_annotations: annotations,
            };
            let collapsed = with_display(stage, display);
            let expanded =
                structured::lifted_lines_expanded(stage, &index, display, &expanded_calls);
            for (lines, controls) in [
                (&collapsed, vec![canonical]),
                (&expanded, vec![canonical, actual]),
            ] {
                assert!(!lines.iter().any(|line| line.text == "} else {"));
                for control in controls {
                    let header = lines
                        .iter()
                        .find(|line| line.entity == control.id && line.text.starts_with("if ("))
                        .unwrap();
                    assert!(header.text.contains("!= 0"), "{}", header.text);
                }
            }
            assert!(
                expanded
                    .iter()
                    .any(|line| line.entity == call.id && line.text.starts_with("// Expanded:"))
            );
        }
    }
    let (ctx, mut workspace) = setup(artifact.clone());
    let canonical_at = EntityRef {
        stage: stage.id,
        entity: canonical.id,
    };
    assert!(workspace.state.navigate(&artifact, canonical_at));
    let before = workspace.state.details(&artifact, canonical_at);
    assert!(Rc::ptr_eq(
        &before,
        &workspace.state.details(&artifact, canonical_at)
    ));
    frame(&ctx, &mut workspace, vec![]);
    workspace.state.toggle_call_expansion(EntityRef {
        stage: stage.id,
        entity: call.id,
    });
    let after = workspace.state.details(&artifact, canonical_at);
    assert!(
        !Rc::ptr_eq(&before, &after),
        "expansion invalidates the prepared detail scope"
    );
    let actual_at = EntityRef {
        stage: stage.id,
        entity: actual.id,
    };
    assert!(workspace.state.navigate(&artifact, actual_at));
    let details = workspace.state.details(&artifact, actual_at);
    assert!(details.lines[0].contains("!= 0"));
    assert!(
        details
            .lines
            .iter()
            .any(|line| line.starts_with("Original predicate"))
    );
    let StructuredKind::If {
        then_branch,
        else_branch,
        ..
    } = &renderer.node(actual.node).unwrap().kind
    else {
        unreachable!()
    };
    for entity in [
        actual.node,
        actual.condition,
        then_branch.id,
        else_branch.id,
    ] {
        assert!(details.references.contains(&EntityRef {
            stage: stage.id,
            entity
        }));
    }
    let expected =
        structured::lifted_lines_expanded(stage, &index, Default::default(), &expanded_calls);
    let visible = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.state.selected.as_ref().unwrap().at, actual_at);
    assert!(visible.controls.iter().any(|row| {
        row["role"] == "decompile-code"
            && row["selected"] == true
            && row["label"]
                .as_str()
                .is_some_and(|label| label.contains("if (") && label.contains("!= 0"))
    }));
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&expected)]
    );
    assert_eq!(rows(&structured::lines(stage)), raw);
    assert_eq!(artifact, original);
}

fn assert_semantic_rows_survive_toggles(stage: &Stage) {
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    for dependencies in [false, true] {
        let mut annotations = super::annotations::annotations(stage);
        for procedure in &program.procedures {
            annotations.extend(
                renderer
                    .render_procedure_body(procedure, dependencies, |_| true)
                    .unwrap()
                    .into_iter()
                    .filter(|row| row.is_structure_annotation)
                    .map(|row| (row.entity, row.text)),
            );
        }
        let full = with_display(
            stage,
            structured::HighDisplayOptions {
                show_state_dependencies: dependencies,
                show_structure_annotations: true,
            },
        );
        let expected: Vec<_> = rows(&full)
            .into_iter()
            .filter(|(entity, _, text)| !annotations.contains(&(*entity, text.clone())))
            .collect();
        assert_eq!(rows(&high(stage, dependencies)), expected);
    }
    let mut signatures = BTreeMap::new();
    for procedure in &program.procedures {
        signatures.insert(
            (
                procedure.id,
                renderer
                    .render_procedure_header(procedure, true, |_| true)
                    .unwrap(),
            ),
            renderer
                .render_procedure_header(procedure, false, |_| true)
                .unwrap(),
        );
    }
    for call in &program.invocations {
        signatures.insert(
            (
                call.id,
                renderer
                    .render_shared_invocation(call, true, |_| true)
                    .unwrap(),
            ),
            renderer
                .render_shared_invocation(call, false, |_| true)
                .unwrap(),
        );
    }
    let tokens: BTreeSet<_> = stage
        .ir
        .edges
        .iter()
        .flat_map(|edge| {
            let target = stage.ir.block(edge.to).unwrap();
            [
                format!(
                    "m{} = m{}; // memory token on e{}",
                    target.memory_parameter.0, edge.memory.0, edge.id.0
                ),
                format!(
                    "q{} = q{}; // effect token on e{}",
                    target.effect_parameter.0, edge.effect.0, edge.id.0
                ),
            ]
        })
        .collect();
    let expected: Vec<_> = rows(&high(stage, true))
        .into_iter()
        .filter(|(_, _, text)| !tokens.contains(text))
        .map(|(entity, depth, text)| {
            let clean = signatures
                .get(&(entity, text.clone()))
                .cloned()
                .unwrap_or_else(|| without_annotations(&text).to_owned());
            (entity, depth, clean)
        })
        .collect();
    assert_eq!(
        rows(&high(stage, false)),
        expected,
        "all semantic rows, source identities, indentation and effect order must survive state toggles"
    );
}

#[test]
fn shared_definitions_and_calls_use_model_text_in_all_display_modes_and_clipboard() {
    let artifact = artifact(SHARED);
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !program.procedures.is_empty(),
        "fixture must really share code"
    );
    assert!(
        program.invocations.len() >= 2,
        "fixture must have multiple callers"
    );
    let renderer = program.render_index(&stage.ir);
    renderer.validate_shared_sources().unwrap();
    let (store, row) = program
        .procedures
        .iter()
        .flat_map(|procedure| {
            renderer
                .render_procedure_body(procedure, false, |_| true)
                .unwrap()
        })
        .find_map(|row| {
            program
                .blocks
                .iter()
                .flat_map(|block| &block.statements)
                .find(|statement| {
                    statement.id == row.entity
                        && statement.inputs.len() == 2
                        && stage
                            .ir
                            .blocks
                            .iter()
                            .flat_map(|block| &block.operations)
                            .any(|operation| {
                                operation.id == statement.operation && operation.opcode == 0x55
                            })
                        && matches!(
                            program.expression(statement.inputs[1]).unwrap().kind,
                            LiftedExpressionKind::WordOp { opcode: 0x01, .. }
                        )
                })
                .map(|statement| (statement, row))
        })
        .expect("the canonical shared definition must store its inline ADD");
    let LiftedExpressionKind::WordOp { arguments, .. } =
        &program.expression(store.inputs[1]).unwrap().kind
    else {
        unreachable!()
    };
    assert_eq!(
        program.expression(arguments[0]).unwrap().kind,
        LiftedExpressionKind::Word(U256::from(1))
    );
    let LiftedExpressionKind::Value(input) = program.expression(arguments[1]).unwrap().kind else {
        panic!("shared ADD must use the original source word")
    };
    assert_eq!(
        row.text,
        format!("evm.sstore(slot = 0x5, value = (0x1 + v{}));", input.0)
    );
    let raw = rows(&structured::lines(stage));
    let reading = copied_text(&high(stage, false));
    assert_eq!(reading.matches("evm.log0(").count(), 1, "{reading}");
    assert_eq!(reading.matches("evm.mstore(").count(), 1, "{reading}");
    assert_eq!(
        copied_text(&structured::lines(stage))
            .matches("evm.log0(")
            .count(),
        2
    );
    assert_semantic_rows_survive_toggles(stage);
    let (ctx, mut workspace) = setup(artifact.clone());
    let mut display = structured::HighDisplayOptions::default();
    for (dependencies, annotations) in [
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ] {
        if dependencies != display.show_state_dependencies {
            click(&ctx, &mut workspace, "Show state dependencies");
        }
        if annotations != display.show_structure_annotations {
            click(&ctx, &mut workspace, "Show structure annotations");
        }
        display = structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            show_structure_annotations: annotations,
        };
        let lines = with_display(stage, display);
        for procedure in &program.procedures {
            let header = renderer
                .render_procedure_header(procedure, dependencies, |_| true)
                .unwrap();
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.entity == procedure.id && line.text == header)
                    .count(),
                1,
            );
        }
        for invocation in &program.invocations {
            let call = renderer
                .render_shared_invocation(invocation, dependencies, |_| true)
                .unwrap();
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.entity == invocation.id && line.text == call)
                    .count(),
                1,
            );
        }
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &lines);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&lines)],
        );
        assert_eq!(rows(&structured::lines(stage)), raw);
    }
    click(&ctx, &mut workspace, "Structured code");
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&structured::lines(stage))],
    );
    let ir_before = labels(&click(&ctx, &mut workspace, "IR"), "decompile-ir");
    assert!(!ir_before.is_empty());
    assert!(
        !frame(&ctx, &mut workspace, vec![])
            .controls
            .iter()
            .any(|row| row["label"] == "Copy structured text")
    );
    click(&ctx, &mut workspace, "Simplified structured code");
    click(&ctx, &mut workspace, "Show state dependencies");
    assert_eq!(
        labels(&click(&ctx, &mut workspace, "IR"), "decompile-ir"),
        ir_before
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&original));
    assert_eq!(artifact, original);
}

#[test]
fn definitions_calls_and_original_noncanonical_primitives_keep_exact_navigation() {
    let artifact = artifact(SHARED);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert!(!program.procedures.is_empty());
    assert!(program.invocations.len() >= 2);
    let (ctx, mut workspace) = setup(artifact.clone());
    for (entity, kind) in program
        .procedures
        .iter()
        .map(|p| (p.id, EntityKind::LiftedProcedure))
        .chain(
            program
                .invocations
                .iter()
                .map(|call| (call.id, EntityKind::LiftedInvocation)),
        )
    {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        assert_eq!(workspace.state.index.entities.get(&at), Some(&kind));
        let origin = artifact
            .origin(at)
            .expect("shared entity has source provenance");
        assert!(!origin.source.is_empty());
        workspace
            .state
            .select_source(&artifact, origin.source[0].pc);
        assert!(
            workspace
                .state
                .source_descendants(&artifact, origin.source[0].pc)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let result = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert!(
            result
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
        let details = workspace.state.details(&artifact, at);
        assert!(details.lines[0].starts_with("Certified shared"));
        assert!(
            details
                .references
                .iter()
                .all(|reference| workspace.state.index.contains(*reference))
        );
    }
    let statements: BTreeSet<_> = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .map(|s| s.operation)
        .collect();
    let pair = program
        .invocations
        .iter()
        .flat_map(|call| &call.bindings)
        .find(|binding| {
            binding.formal != binding.actual
                && statements.contains(&binding.formal)
                && statements.contains(&binding.actual)
                && stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(|op| op.id == binding.formal && op.opcode == 0x55)
        })
        .expect("exercise a noncanonical caller's effectful source primitive");
    let canonical = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|s| s.operation == pair.formal)
        .unwrap();
    let noncanonical = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|s| s.operation == pair.actual)
        .unwrap();
    let visible = high(stage, false);
    assert!(!visible.iter().any(|row| row.entity == noncanonical.id));
    let canonical_route = if visible.iter().any(|row| row.entity == canonical.id) {
        "Simplified structured code"
    } else {
        "IR"
    };
    for (entity, expected_route) in [
        (canonical.id, canonical_route),
        (noncanonical.id, "IR"),
        (pair.formal, "IR"),
        (pair.actual, "IR"),
    ] {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        let source = artifact.origin(at).unwrap().source[0].pc;
        workspace.state.select_source(&artifact, source);
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let result = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.state.selected.as_ref().unwrap().at, at);
        assert_eq!(workspace.views.route_label(), expected_route);
        let role = if expected_route == "IR" {
            "decompile-ir"
        } else {
            "decompile-code"
        };
        assert!(
            result
                .controls
                .iter()
                .any(|row| row["role"] == role && row["selected"] == true)
        );
    }
}

#[test]
fn malformed_shared_plans_quarantine_omission_and_reject_publication_for_every_status() {
    let base = artifact(SHARED);
    let stage = base.current().unwrap();
    assert!(!stage.ir.lifted.as_ref().unwrap().procedures.is_empty());
    let mut unshared = stage.clone();
    let program = unshared.ir.lifted.as_mut().unwrap();
    program.procedures.clear();
    program.invocations.clear();
    for mutation in [
        "body",
        "source",
        "binding",
        "owner",
        "continuation",
        "duplicate_id",
        "unsorted_bindings",
        "missing_procedure",
    ] {
        for status in [ValidationStatus::Passed, ValidationStatus::Failed] {
            let mut changed = base.clone();
            let current = changed.current_stage.unwrap();
            let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
            stage.validation.status = status;
            let program = stage.ir.lifted.as_mut().unwrap();
            let missing = EntityId(u64::MAX);
            match mutation {
                "body" => program.procedures[0].body = missing,
                "source" => program.invocations[0].source = missing,
                "binding" => {
                    program
                        .invocations
                        .iter_mut()
                        .find(|call| !call.bindings.is_empty())
                        .expect("fixture needs nonidentity source bindings")
                        .bindings[0]
                        .actual = missing;
                }
                "owner" => program.invocations[0].owner = Some(missing),
                "continuation" => program.invocations[0].continuation = Some(missing),
                "duplicate_id" => program.invocations[1].id = program.invocations[0].id,
                "unsorted_bindings" => {
                    program
                        .invocations
                        .iter_mut()
                        .find(|call| call.bindings.len() >= 2)
                        .expect("fixture needs multiple nonidentity bindings")
                        .bindings
                        .reverse();
                }
                "missing_procedure" => program.invocations[0].procedure = missing,
                _ => unreachable!(),
            }
            assert!(
                stage
                    .ir
                    .lifted
                    .as_ref()
                    .unwrap()
                    .validate_shared_sources(&stage.ir, |_| true)
                    .is_err()
            );
            for dependencies in [false, true] {
                for annotations in [false, true] {
                    let display = structured::HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    };
                    assert_eq!(
                        rows(&with_display(stage, display)),
                        rows(&with_display(&unshared, display)),
                        "{mutation} {status:?}"
                    );
                }
            }
            assert!(
                model::check_artifact(&changed)
                    .unwrap_err()
                    .contains("shared procedure sources")
            );
            let (ctx, mut workspace) = setup(base.clone());
            let before = click(&ctx, &mut workspace, "Copy structured text").copied;
            deliver(&mut workspace, changed, 11);
            assert_eq!(workspace.artifact.as_ref(), Some(&base));
            assert_eq!(
                click(&ctx, &mut workspace, "Copy structured text").copied,
                before
            );
        }
    }
}

#[test]
fn refused_preparation_keeps_the_complete_original_structured_witness() {
    let artifact = artifact(SHARED);
    let stage = artifact.current().unwrap();
    assert!(!stage.ir.lifted.as_ref().unwrap().procedures.is_empty());
    let index = crate::decompile::index::StageIndex::with_render_work_limit(&stage.ir, 0);
    let expected = rows(&structured::lines(stage));
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let display = structured::HighDisplayOptions {
                show_state_dependencies: dependencies,
                show_structure_annotations: annotations,
            };
            assert_eq!(
                rows(&structured::lifted_lines_indexed(stage, &index, display)),
                expected
            );
        }
    }
}

#[test]
fn multiblock_definitions_restore_exact_edge_state_dependencies_under_the_state_toggle() {
    let artifact = artifact(SHARED_LOOP.trim());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert!(!program.procedures.is_empty());
    let renderer = program.render_index(&stage.ir);
    let mut checked = 0;
    for procedure in &program.procedures {
        let root = renderer.node(procedure.body).unwrap();
        let mut blocks = BTreeSet::new();
        let mut edges = BTreeSet::new();
        structured::visit(root, &mut |node| match node.kind {
            StructuredKind::Block { block } => {
                blocks.insert(block);
            }
            StructuredKind::Transfer { edge } => {
                edges.insert(edge);
            }
            StructuredKind::While {
                exit_edge: Some(edge),
                ..
            } => {
                edges.insert(edge);
            }
            _ => {}
        });
        if blocks.len() < 2 || edges.is_empty() {
            continue;
        }
        let reading = renderer
            .render_procedure_body(procedure, false, |_| true)
            .unwrap();
        let detailed = renderer
            .render_procedure_body(procedure, true, |_| true)
            .unwrap();
        let shown = high(stage, true);
        for id in edges {
            let edge = stage.ir.edges.iter().find(|edge| edge.id == id).unwrap();
            let target = stage.ir.block(edge.to).unwrap();
            for text in [
                format!(
                    "m{} = m{}; // memory token on e{}",
                    target.memory_parameter.0, edge.memory.0, id.0
                ),
                format!(
                    "q{} = q{}; // effect token on e{}",
                    target.effect_parameter.0, edge.effect.0, id.0
                ),
            ] {
                assert!(
                    detailed.iter().any(|row| row.text == text),
                    "definition edge lost: {text}"
                );
                assert!(
                    shown.iter().any(|row| row.text == text),
                    "UI edge lost: {text}"
                );
                assert!(!reading.iter().any(|row| row.text == text));
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 4,
        "require actual multi-block definition memory/effect edges"
    );
    assert_semantic_rows_survive_toggles(stage);
}

#[test]
fn a_new_publication_with_the_same_artifact_id_replaces_prepared_shared_rows() {
    let base = artifact(SHARED);
    let stage_id = base.current_stage.unwrap();
    let program = base.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(!program.procedures.is_empty());
    let removed: BTreeSet<_> = program
        .procedures
        .iter()
        .map(|p| p.id)
        .chain(program.invocations.iter().map(|call| call.id))
        .collect();
    let old_entity = EntityRef {
        stage: stage_id,
        entity: program.procedures[0].id,
    };
    let (ctx, mut workspace) = setup(base.clone());
    let before = click(&ctx, &mut workspace, "Copy structured text").copied;
    let old_index = workspace.state.index.stage(stage_id);

    // The same input namespace can publish the original certified high witness
    // without sharing. Remove only the sharing entities and their history rows.
    let mut replacement = base.clone();
    let current = replacement
        .stages
        .iter_mut()
        .find(|stage| stage.id == stage_id)
        .unwrap();
    let projection = current.ir.lifted.as_mut().unwrap();
    projection.procedures.clear();
    projection.invocations.clear();
    replacement.mappings.origins.retain(|origin| {
        origin.entity.stage != stage_id || !removed.contains(&origin.entity.entity)
    });
    for event in &mut replacement.events {
        event.created.retain(|entity| !removed.contains(entity));
        event
            .outputs
            .retain(|at| at.stage != stage_id || !removed.contains(&at.entity));
    }
    model::check_artifact(&replacement).unwrap();
    assert_eq!(replacement.id, base.id);
    let expected = copied_text(&high(replacement.current().unwrap(), false));
    deliver(&mut workspace, replacement.clone(), 11);
    assert!(!Rc::ptr_eq(
        &old_index,
        &workspace.state.index.stage(stage_id)
    ));
    assert!(!workspace.state.index.contains(old_entity));
    let after = click(&ctx, &mut workspace, "Copy structured text").copied;
    assert_ne!(after, before);
    assert_eq!(after.as_slice(), std::slice::from_ref(&expected));
    assert_eq!(expected.matches("evm.log0(").count(), 2);
    assert_eq!(workspace.artifact.as_ref(), Some(&replacement));
}

#[test]
fn a_late_preparation_refusal_publishes_no_partial_definitions_or_calls() {
    let artifact = artifact(SHARED);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert!(!program.procedures.is_empty());
    let procedure = &program.procedures[0];
    let mut enough = 1_u64;
    for _ in 0..64 {
        let index = crate::decompile::index::StageIndex::with_render_work_limit(&stage.ir, enough);
        if index.shared.header(procedure.id, false).is_some() {
            break;
        }
        enough = enough.saturating_mul(2);
    }
    let complete = crate::decompile::index::StageIndex::with_render_work_limit(&stage.ir, enough);
    assert!(complete.shared.header(procedure.id, false).is_some());
    let mut below = 0_u64;
    while enough - below > 1 {
        let middle = below + (enough - below) / 2;
        let index = crate::decompile::index::StageIndex::with_render_work_limit(&stage.ir, middle);
        if index.shared.header(procedure.id, false).is_some() {
            enough = middle;
        } else {
            below = middle;
        }
    }
    // This allowance admits the source proof and a complete first body. The
    // full cache's final formatting request is refused, after useful work.
    let work = crate::decompile::lifted::work::Work::new(below);
    let renderer = program.render_index_with_budget(&stage.ir, |amount| work.reserve(amount));
    renderer.validate_shared_sources().unwrap();
    renderer
        .render_procedure_header(procedure, false, |amount| work.reserve(amount))
        .unwrap();
    renderer
        .render_procedure_body(procedure, false, |amount| work.reserve(amount))
        .unwrap();
    let refused = crate::decompile::index::StageIndex::with_render_work_limit(&stage.ir, below);
    assert!(refused.shared.refused());
    assert!(refused.shared.definitions().is_empty());
    assert!(
        program
            .invocations
            .iter()
            .all(|call| refused.shared.invocation(call.source, false).is_none())
    );
    let mut source = stage.clone();
    let projection = source.ir.lifted.as_mut().unwrap();
    projection.procedures.clear();
    projection.invocations.clear();
    assert_eq!(
        rows(&structured::lifted_lines_indexed(
            stage,
            &refused,
            structured::HighDisplayOptions::default()
        )),
        rows(&high(&source, false))
    );
}

#[test]
fn inspector_virtualizes_exact_noncanonical_binding_pairs() {
    let artifact = artifact(SHARED);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert!(!program.procedures.is_empty());
    let call = program
        .invocations
        .iter()
        .max_by_key(|call| call.bindings.len())
        .unwrap();
    assert!(
        call.bindings.len() > 10,
        "exercise more source pairs than fit in the panel"
    );
    let (ctx, mut workspace) = setup(artifact.clone());
    workspace.inspector = true;
    assert!(workspace.state.navigate(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: call.id
        }
    ));
    let mut result = frame(&ctx, &mut workspace, vec![]);
    if labels(&result, "decompile-procedure-binding").is_empty() {
        click(&ctx, &mut workspace, "Procedure source bindings");
        result = frame(&ctx, &mut workspace, vec![]);
    }
    let pairs = labels(&result, "decompile-procedure-binding");
    assert!(!pairs.is_empty() && pairs.len() <= 10);
    for (text, binding) in pairs.iter().zip(&call.bindings) {
        assert_eq!(
            *text,
            format!("e{} → e{}", binding.formal.0, binding.actual.0)
        );
    }
    let details = workspace.state.details(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: call.id,
        },
    );
    assert!(
        details.lines.len() <= 5,
        "the full mapping belongs in the virtual list"
    );
    for binding in &call.bindings {
        assert!(details.references.contains(&EntityRef {
            stage: stage.id,
            entity: binding.formal
        }));
        assert!(details.references.contains(&EntityRef {
            stage: stage.id,
            entity: binding.actual
        }));
    }
}

#[test]
fn arithmetic_templates_keep_bounded_local_rows_original_evaluation_sites_and_exact_navigation() {
    let artifact = artifact(ARITHMETIC.trim());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    let procedures: Vec<_> = program
        .procedures
        .iter()
        .filter(|procedure| {
            procedure.kind == SharedProcedureKind::PureArithmetic
                && renderer.expression(procedure.body).is_some()
        })
        .collect();
    assert!(
        !procedures.is_empty(),
        "long arithmetic DAGs need explicit classification"
    );
    let calls: Vec<_> = program
        .invocations
        .iter()
        .filter(|call| renderer.expression(call.source).is_some())
        .collect();
    assert!(
        calls.len() >= 2,
        "the two original arithmetic evaluation sites must survive"
    );
    let original = artifact.clone();
    let (ctx, mut workspace) = setup(artifact.clone());
    let mut display = structured::HighDisplayOptions::default();
    for (dependencies, annotations) in [
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ] {
        if dependencies != display.show_state_dependencies {
            click(&ctx, &mut workspace, "Show state dependencies");
        }
        if annotations != display.show_structure_annotations {
            click(&ctx, &mut workspace, "Show structure annotations");
        }
        display = structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            show_structure_annotations: annotations,
        };
        let lines = with_display(stage, display);
        let text = copied_text(&lines);
        assert_eq!(text.matches("evm.mstore(").count(), 2);
        assert_eq!(text.matches("evm.return(").count(), 1);
        for procedure in &procedures {
            let header = renderer
                .render_procedure_header(procedure, dependencies, |_| true)
                .unwrap();
            assert!(header.starts_with(&format!("procedure pure_arithmetic_{}(", procedure.id.0)));
            assert!(header.contains("-> word256 {"));
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.entity == procedure.id && line.text == header)
                    .count(),
                1
            );
            let body = renderer
                .render_procedure_body(procedure, dependencies, |_| true)
                .unwrap();
            assert!(body.len() >= 5);
            assert!(body.iter().all(|row| row.text.len() < 256
                && !row.text.contains("evm.mstore(")
                && !row.text.contains("evm.return(")));
            assert_eq!(
                body.last().unwrap().text,
                format!("yield x{};", procedure.body.0)
            );
            for row in body {
                assert!(
                    lines
                        .iter()
                        .any(|line| line.entity == row.entity && line.text == row.text)
                );
            }
        }
        for call in &calls {
            let text = renderer
                .render_shared_invocation(call, dependencies, |_| true)
                .unwrap();
            assert!(
                lines.iter().any(|line| line.text.contains(&text)),
                "call lost from its source evaluation site: {text}"
            );
        }
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &lines);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&lines)]
        );
    }
    for entity in procedures
        .iter()
        .map(|procedure| procedure.id)
        .chain(calls.iter().map(|call| call.id))
    {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        let source = artifact.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let selected = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert_eq!(workspace.state.selected.as_ref().unwrap().at, at);
        assert!(
            selected
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
        let details = workspace.state.details(&artifact, at);
        assert!(details.lines[0].starts_with("Certified pure_arithmetic"));
        assert!(
            details
                .references
                .iter()
                .all(|reference| workspace.state.index.contains(*reference))
        );
    }
    let local = renderer
        .render_procedure_body(procedures[0], false, |_| true)
        .unwrap()
        .into_iter()
        .find(|row| row.entity != procedures[0].body && renderer.expression(row.entity).is_some())
        .unwrap();
    assert!(workspace.state.navigate(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: local.entity
        }
    ));
    let selected = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(
        selected
            .controls
            .iter()
            .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
    );
    assert_semantic_rows_survive_toggles(stage);
    assert_eq!(artifact, original);
}

#[test]
fn nested_arithmetic_calls_keep_finite_definitions_clipboard_and_exact_invocation_navigation() {
    let artifact = artifact(include_str!("../../../../../evm-decompile/tests/fixtures/shared-procedures/nested-arithmetic-inputs.hex").trim());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    assert!(program.procedures.len() >= 2);
    let nested: BTreeSet<_> = program
        .procedures
        .iter()
        .flat_map(|procedure| {
            let body = renderer
                .render_procedure_body(procedure, false, |_| true)
                .unwrap();
            assert!(
                body.len() < 100,
                "finite definitions must not recursively expand nested calls"
            );
            body.into_iter()
                .filter_map(|row| {
                    program
                        .invocations
                        .iter()
                        .find(|call| call.id == row.entity && call.procedure != procedure.id)
                        .map(|call| {
                            assert!(
                                row.text
                                    .contains(&format!("pure_arithmetic_{}(", call.procedure.0))
                            );
                            call.id
                        })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        !nested.is_empty(),
        "exercise an actual call inside another arithmetic definition"
    );
    let (ctx, mut workspace) = setup(artifact.clone());
    let mut display = structured::HighDisplayOptions::default();
    for (dependencies, annotations) in [
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ] {
        if dependencies != display.show_state_dependencies {
            click(&ctx, &mut workspace, "Show state dependencies");
        }
        if annotations != display.show_structure_annotations {
            click(&ctx, &mut workspace, "Show structure annotations");
        }
        display = structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            show_structure_annotations: annotations,
        };
        let lines = with_display(stage, display);
        for call in &nested {
            assert_eq!(lines.iter().filter(|line| line.entity == *call).count(), 1);
        }
        let text = copied_text(&lines);
        assert_eq!(text.matches("evm.mstore(").count(), 3);
        assert_eq!(text.matches("evm.return(").count(), 1);
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &lines);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [text]
        );
    }
    for entity in nested {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        let source = artifact.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let selected = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert_eq!(workspace.state.selected.as_ref().unwrap().at, at);
        assert!(
            selected
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
        assert!(
            workspace.state.details(&artifact, at).lines[0]
                .starts_with("Certified pure_arithmetic invocation")
        );
    }
    assert_semantic_rows_survive_toggles(stage);
}

#[test]
fn expand_one_certified_call_in_place_preserves_other_calls_copy_and_publication_identity() {
    let artifact = artifact(SHARED);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = crate::decompile::index::StageIndex::new(&stage.ir);
    let call = program
        .invocations
        .iter()
        .find(|call| index.node(&stage.ir, call.source).is_some())
        .unwrap();
    let other = program
        .invocations
        .iter()
        .find(|other| other.id != call.id && index.node(&stage.ir, other.source).is_some())
        .unwrap();
    let before = high(stage, false);
    let expected = structured::lifted_lines_expanded(
        stage,
        &index,
        Default::default(),
        &BTreeSet::from([call.id]),
    );
    assert!(
        expected
            .iter()
            .any(|line| line.entity == call.id && line.text.starts_with("// Expanded:"))
    );
    assert!(
        expected
            .iter()
            .any(|line| line.entity == other.id && !line.text.starts_with("// Expanded:"))
    );
    assert!(expected.len() > before.len());
    let source = index.node(&stage.ir, call.source).unwrap();
    let mut source_blocks = BTreeSet::new();
    structured::visit(source, &mut |node| {
        if let StructuredKind::Block { block } = node.kind {
            source_blocks.insert(block);
        }
    });
    let renderer = program.render_base_index_with_budget(&stage.ir, |_| true);
    let actual_store = program
        .blocks
        .iter()
        .filter(|block| source_blocks.contains(&block.block))
        .flat_map(|block| &block.statements)
        .find(|statement| {
            index
                .operation(&stage.ir, statement.operation)
                .is_some_and(|operation| operation.opcode == 0x55)
        })
        .unwrap();
    let actual_text = renderer
        .render_statement_with_dependencies(actual_store, false, |_| true)
        .unwrap();
    assert!(
        expected
            .iter()
            .any(|line| line.entity == actual_store.id && line.text == actual_text)
    );
    let (ctx, mut workspace) = setup(artifact.clone());
    workspace.state.navigate(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: call.id,
        },
    );
    frame(&ctx, &mut workspace, vec![]);
    let expanded = click(&ctx, &mut workspace, "Expand call here");
    assert_painted(&expanded, &expected);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&expected)]
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
    let collapsed = click(&ctx, &mut workspace, "Collapse call here");
    assert_painted(&collapsed, &before);
    click(&ctx, &mut workspace, "Expand call here");
    deliver(&mut workspace, artifact.clone(), 11);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&before)]
    );
    let stale = structured::lifted_lines_expanded(
        stage,
        &index,
        Default::default(),
        &BTreeSet::from([EntityId(u64::MAX)]),
    );
    assert_eq!(rows(&stale), rows(&before));
}

#[test]
fn expanding_a_nested_arithmetic_call_uses_its_original_source_expression() {
    let artifact = artifact(ARITHMETIC);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = crate::decompile::index::StageIndex::new(&stage.ir);
    let call = program
        .invocations
        .iter()
        .find(|call| index.expression(&stage.ir, call.source).is_some())
        .unwrap();
    let baseline = high(stage, false);
    let expected = structured::lifted_lines_expanded(
        stage,
        &index,
        Default::default(),
        &BTreeSet::from([call.id]),
    );
    assert_ne!(copied_text(&expected), copied_text(&baseline));
    let before_rows: Vec<_> = baseline
        .iter()
        .filter(|line| index.shared.row_matches(line.entity, call.id))
        .collect();
    assert!(!before_rows.is_empty());
    assert!(
        before_rows
            .iter()
            .any(|line| line.text.contains("pure_arithmetic_"))
    );
    let mut renderer = program.render_index(&stage.ir);
    renderer
        .expand_shared_sources(&BTreeSet::from([call.source]), |_| true)
        .unwrap();
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| before_rows.iter().any(|line| line.entity == statement.id))
        .unwrap();
    let caller_text = renderer
        .render_statement_with_dependencies(statement, false, |_| true)
        .unwrap();
    assert!(
        expected
            .iter()
            .any(|line| line.entity == statement.id && line.text == caller_text)
    );
    let (ctx, mut workspace) = setup(artifact.clone());
    workspace.state.navigate(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: call.id,
        },
    );
    frame(&ctx, &mut workspace, vec![]);
    assert_painted(&click(&ctx, &mut workspace, "Expand call here"), &expected);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&expected)]
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
}
