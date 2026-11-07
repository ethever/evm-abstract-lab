//! Certified sharing crosses publication, cached code, clipboard and navigation.
use super::*;
use crate::decompile::model::EntityKind;
use evm_abstract_protocol::decompile_model::ValidationStatus;
use std::collections::BTreeMap;

// Two different caller words enter the same two-block helper. Each call writes
// memory/storage and emits LOG0, then returns two words to a distinct caller.
const SHARED: &str = concat!(
    "60075f35601e565b6001556002556016602035601e565b60035560045500",
    "5b80604052805f5560206040a0600554600101600555600654600755603d565b806001019156",
);
const SHARED_LOOP: &str = include_str!("../../../../tests/fixtures/decompile/shared-loop.hex");
// The helper's false arm has only an empty block and m/q transfers. Two callers
// retain the live word result, and the true arm has observable memory/log/store.
const SHARED_TOKEN_ELSE: &str = concat!(
    "61000a60003561001d565b60015561001860203561001d565b60025500",
    "5b80604052806000558061002f57610041565b8060605260206040a0",
    "80600355806004555b6001019056",
);

fn with_display(stage: &Stage, display: structured::HighDisplayOptions) -> Vec<structured::Line> {
    structured::lifted_lines_indexed(
        stage,
        &crate::decompile::index::StageIndex::new(&stage.ir),
        display,
    )
}

#[test]
fn cached_shared_else_follows_substantive_content_in_all_display_modes() {
    let artifact = artifact(SHARED_TOKEN_ELSE);
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(program.procedures.len(), 1);
    assert_eq!(program.invocations.len(), 2);
    let renderer = program.render_index(&stage.ir);
    let procedure = &program.procedures[0];
    let branch = renderer.node(procedure.body).unwrap();
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
                .filter(|(entity, depth, text)| *entity == branch.id
                    && *depth == 1
                    && text == "} else {")
                .count(),
            usize::from(dependencies),
            "{rendered:#?}"
        );
        assert!(rendered.iter().any(|(entity, depth, text)| {
            *entity == branch.id && *depth == 1 && text.starts_with("if (")
        }));
        assert!(rendered.contains(&(branch.id, 1, "}".into())));
        for token in &tokens {
            assert_eq!(rendered.contains(token), dependencies, "{rendered:#?}");
        }
        if !dependencies {
            assert!(
                !rendered
                    .iter()
                    .any(|(entity, _, _)| context.contains(entity))
            );
        }
        let copied = copied_text(&lines);
        assert_eq!(
            copied.matches("} else {").count(),
            usize::from(dependencies)
        );
        assert_eq!(copied.matches("evm.log0(").count(), 1);
        assert_eq!(copied.matches("evm.sstore(slot = 0x3,").count(), 1);
        assert_eq!(copied.matches("evm.sstore(slot = 0x4,").count(), 1);
        assert_eq!(rows(&structured::lines(stage)), raw);
    }
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
