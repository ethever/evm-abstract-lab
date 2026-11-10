//! Native consumer tests use actual published engine artifacts and UI controls.
use super::{Workspace, deliver, fixture, model};
use crate::controls;
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    DecompileArtifact, DecompileRequest, JobState,
    decompile_model::{
        EntityRef, LiftedExpressionKind, LiftedRelation, StageKind, StructuredKind, Terminator,
    },
};
use evm_abstract_server::jobs::{Config, Pool};
use serde_json::Value;
use std::{
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

const BOOLEAN: &str = "6020355f3510151515600f575f5ffd5b00";

fn artifact(bytecode: &str) -> DecompileArtifact {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 1,
    })
    .unwrap();
    let task = pool
        .submit_decompile(DecompileRequest {
            bytecode: bytecode.into(),
            ..DecompileRequest::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = pool.decompile_status(task.id).unwrap();
        if snapshot.state == JobState::Completed {
            break;
        }
        assert!(Instant::now() < deadline, "{snapshot:?}");
        thread::sleep(Duration::from_millis(2));
    }
    let artifact = pool
        .decompile_result(task.id)
        .unwrap()
        .artifact
        .as_ref()
        .clone();
    pool.shutdown().unwrap();
    model::check_artifact(&artifact).unwrap();
    assert_eq!(
        artifact.current().unwrap().kind,
        StageKind::Lifted,
        "{:?}",
        artifact.diagnostics
    );
    artifact
}

fn frame(ctx: &Context, workspace: &mut Workspace, events: Vec<Event>) -> Vec<Value> {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            controls::begin_observation(ui.ctx(), true);
            crate::decompile::rows::begin_frame(ui.ctx());
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    serde_json::from_str(&controls::observed(ctx).unwrap()).unwrap()
}

fn click(ctx: &Context, workspace: &mut Workspace, label: &str) -> Vec<Value> {
    let controls = frame(ctx, workspace, vec![]);
    let control = controls
        .iter()
        .find(|control| control["label"] == label)
        .unwrap_or_else(|| panic!("missing {label}: {controls:?}"));
    let [left, top, right, bottom]: [f32; 4] =
        serde_json::from_value(control["rect"].clone()).unwrap();
    let pos = Pos2::new((left + right) * 0.5, (top + bottom) * 0.5);
    for pressed in [true, false] {
        frame(
            ctx,
            workspace,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    frame(ctx, workspace, vec![])
}

fn code(controls: &[Value]) -> String {
    controls
        .iter()
        .filter(|control| control["role"] == "decompile-code")
        .map(|control| control["label"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn workspace(artifact: DecompileArtifact) -> (Context, Workspace) {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = Workspace::default();
    workspace.form.open = false;
    workspace.inspector = false;
    deliver(&mut workspace, artifact, 10);
    frame(&ctx, &mut workspace, vec![]);
    (ctx, workspace)
}

#[test]
fn lifted_default_renders_actual_unsigned_predicate_and_raw_route_keeps_iszero_chain() {
    let artifact = artifact(BOOLEAN);
    let raw_stage = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap()
        .id;
    let (ctx, mut workspace) = workspace(artifact);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    let high = code(&frame(&ctx, &mut workspace, vec![]));
    assert!(high.contains(" >= "), "{high}");
    assert!(
        !high.contains("iszero") && !high.contains("ISZERO"),
        "{high}"
    );
    assert!(
        high.contains("require(") && high.contains("evm.stop") && !high.contains("evm.revert"),
        "{high}"
    );
    let raw = code(&click(&ctx, &mut workspace, "Structured code"));
    assert!(raw.contains("evm.iszero"), "{raw}");
    assert!(!raw.contains(" >= "), "{raw}");
    let high_again = code(&click(&ctx, &mut workspace, "Simplified structured code"));
    assert_eq!(high_again, high);
    let artifact = workspace.artifact.as_ref().unwrap();
    workspace.state.set_stage(artifact, raw_stage);
    let raw = code(&click(&ctx, &mut workspace, "Structured code"));
    assert!(raw.contains("evm.iszero"), "{raw}");
}

#[test]
fn semantic_expression_and_control_navigation_roundtrip_through_source_and_inspector() {
    let artifact = artifact(BOOLEAN);
    let current = artifact.current().unwrap();
    let program = current.ir.lifted.as_ref().unwrap();
    let predicate = program
        .expressions
        .iter()
        .find(|expression| {
            matches!(
                expression.kind,
                LiftedExpressionKind::Compare {
                    relation: LiftedRelation::Uge,
                    ..
                }
            )
        })
        .unwrap();
    let expression = EntityRef {
        stage: current.id,
        entity: predicate.id,
    };
    let control = EntityRef {
        stage: current.id,
        entity: program.controls[0].id,
    };
    let source = artifact.origin(expression).unwrap().source[0].pc;
    let (ctx, mut workspace) = workspace(artifact.clone());
    assert!(workspace.state.navigate(&artifact, expression));
    let visible = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "IR");
    assert!(visible.iter().any(|row| row["role"] == "decompile-ir"
        && row["selected"] == true
        && row["label"].as_str().unwrap().contains("Uge")));
    let details = workspace.state.details(&artifact, expression);
    assert!(details.lines.iter().any(|line| line.contains(" >= ")));
    assert!(
        details
            .references
            .iter()
            .all(|at| workspace.state.index.contains(*at))
    );
    workspace.state.select_source(&artifact, source);
    assert!(
        workspace
            .state
            .source_descendants(&artifact, source)
            .contains(&expression)
    );
    assert!(workspace.state.navigate(&artifact, control));
    let visible = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(visible.iter().any(|row| row["role"] == "decompile-code"
        && row["selected"] == true
        && row["label"].as_str().unwrap().contains(" >= ")));
    workspace.state.history(&artifact, false);
    frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.state.selected.as_ref().unwrap().at, expression);
    assert_eq!(workspace.views.route_label(), "IR");
}

#[test]
fn accepted_lifted_republication_rebuilds_entity_details_and_source_indices() {
    let artifact = artifact(BOOLEAN);
    let current = artifact.current().unwrap();
    let expression = EntityRef {
        stage: current.id,
        entity: current.ir.lifted.as_ref().unwrap().expressions[0].id,
    };
    let source = artifact.origin(expression).unwrap().source[0].pc;
    let (ctx, mut workspace) = workspace(artifact.clone());
    let old_index = workspace.state.index.stage(expression.stage);
    let old_details = workspace.state.details(&artifact, expression);
    let old_sources = workspace.state.source_descendants(&artifact, source);
    let old_code = code(&frame(&ctx, &mut workspace, vec![]));
    deliver(&mut workspace, artifact.clone(), 11);
    assert!(!Rc::ptr_eq(
        &old_index,
        &workspace.state.index.stage(expression.stage)
    ));
    assert!(!Rc::ptr_eq(
        &old_details,
        &workspace.state.details(&artifact, expression)
    ));
    assert!(!Rc::ptr_eq(
        &old_sources,
        &workspace.state.source_descendants(&artifact, source)
    ));
    assert_eq!(code(&frame(&ctx, &mut workspace, vec![])), old_code);
    assert_eq!(workspace.state.publication.unwrap().job.0, 11);
    assert_eq!(
        workspace.state.publication.unwrap().version,
        fixture::version(1)
    );
}

#[test]
fn lifted_masks_faults_and_unresolved_boundaries_are_visible() {
    for (bytecode, required) in [
        ("5f3560ff165f5260205ff3", " & "),
        ("50", "not completed"),
        ("5f3556", "unresolved"),
        ("5f56", "invalid"),
    ] {
        let (ctx, mut workspace) = workspace(artifact(bytecode));
        let high = code(&frame(&ctx, &mut workspace, vec![]));
        assert!(high.contains(required), "{bytecode}: {high}");
    }
}

fn assert_residual_jump(bytecode: &str, qualifier: &str, partial: bool) {
    let artifact = artifact(bytecode);
    let stage = artifact.current().unwrap();
    let block = stage
        .ir
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, Terminator::Jump { .. }))
        .unwrap();
    let Terminator::Jump {
        target: raw_target, ..
    } = block.terminator
    else {
        unreachable!()
    };
    let program = stage.ir.lifted.as_ref().unwrap();
    let semantic = program.block(block.id).unwrap();
    let target = semantic.target.unwrap();
    let expected = program
        .render_index(&stage.ir)
        .render_expression(target, |_| true)
        .unwrap();
    assert_ne!(expected, format!("v{}", raw_target.0));
    assert!(
        semantic
            .statements
            .iter()
            .all(|statement| !statement.outputs.contains(&raw_target))
    );
    assert!(
        !artifact
            .origin(EntityRef {
                stage: stage.id,
                entity: target
            })
            .unwrap()
            .source
            .is_empty()
    );
    let mut has_boundary = false;
    crate::decompile::structured::visit(&stage.ir.structured.as_ref().unwrap().root, &mut |node| {
        has_boundary |= match node.kind {
            StructuredKind::PartialExit { block: owner } if partial => owner == block.id,
            StructuredKind::Exit { block: owner } if !partial => owner == block.id,
            _ => false,
        };
    });
    assert!(
        has_boundary,
        "exercise the source-backed residual AST boundary"
    );
    let (ctx, mut workspace) = workspace(artifact.clone());
    let high = code(&frame(&ctx, &mut workspace, vec![]));
    assert!(
        high.contains(&format!("jump {expected} via []")),
        "{bytecode}: {high}"
    );
    assert!(high.contains(qualifier), "{bytecode}: {high}");
    assert!(
        !high.contains(&format!("v{}", raw_target.0)),
        "absorbed raw target remains undefined: {high}"
    );
    let raw = code(&click(&ctx, &mut workspace, "Structured code"));
    assert!(
        raw.contains(&format!("jump v{} via []", raw_target.0)),
        "{raw}"
    );
    assert!(raw.contains(qualifier), "{raw}");
}

#[test]
fn lifted_residual_computed_jump_uses_typed_target_and_keeps_partial_boundary() {
    assert_residual_jump("5f3560010156", "unresolved_target (partial)", true);
}

#[test]
fn lifted_residual_invalid_jump_uses_constant_target_and_keeps_exception() {
    assert_residual_jump("60ff56", "exceptional_invalid_target", false);
}

#[test]
fn lifted_residual_conditional_jump_uses_typed_target_and_simplified_predicate() {
    // The true arm has an unresolved computed target; the false arm either
    // reaches STOP or falls off the runtime. Both must retain the true exit.
    for suffix in ["00", ""] {
        let artifact = artifact(&format!("6020355f351015151560403560010157{suffix}"));
        let stage = artifact.current().unwrap();
        let block = stage
            .ir
            .blocks
            .iter()
            .find(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .unwrap();
        let Terminator::Branch {
            target: raw_target, ..
        } = block.terminator
        else {
            unreachable!()
        };
        let program = stage.ir.lifted.as_ref().unwrap();
        let semantic = program.block(block.id).unwrap();
        let renderer = program.render_index(&stage.ir);
        let target = renderer
            .render_expression(semantic.target.unwrap(), |_| true)
            .unwrap();
        let predicate = renderer
            .render_expression(semantic.condition.unwrap(), |_| true)
            .unwrap();
        assert!(predicate.contains(" >= "), "{predicate}");
        assert!(target.contains(" + "), "{target}");
        let (ctx, mut workspace) = workspace(artifact.clone());
        let high = code(&frame(&ctx, &mut workspace, vec![]));
        assert!(high.contains(&format!("if ({predicate})")), "{high}");
        let boundary = if suffix.is_empty() {
            format!("jump {target} via []")
        } else {
            format!("branch {predicate}: jump {target} via []")
        };
        assert!(high.contains(&boundary), "{suffix:?}: {high}");
        assert!(high.contains("unresolved_target (partial)"), "{high}");
        assert!(
            !high.contains("iszero") && !high.contains(&format!("v{}", raw_target.0)),
            "{high}"
        );
        if suffix.is_empty() {
            assert_eq!(
                high.matches("JUMPI false continuation: end of code")
                    .count(),
                1,
                "{high}"
            );
        } else {
            assert!(high.contains("else e"), "{high}");
        }
        let raw = code(&click(&ctx, &mut workspace, "Structured code"));
        assert!(
            raw.contains("evm.iszero") && raw.contains("evm.add"),
            "{raw}"
        );
        assert!(
            crate::decompile::structured::lines(stage)
                .iter()
                .all(|line| raw.contains(&line.text)),
            "raw route changed: {raw}"
        );
    }
}

fn assert_final_branch(bytecode: &str, qualifier: &str) {
    let artifact = artifact(bytecode);
    let stage = artifact.current().unwrap();
    let block = stage
        .ir
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, Terminator::Branch { .. }))
        .unwrap();
    let Terminator::Branch {
        condition: raw_condition,
        target: raw_target,
        else_edge,
        ..
    } = block.terminator
    else {
        unreachable!()
    };
    assert!(else_edge.is_none());
    let program = stage.ir.lifted.as_ref().unwrap();
    let semantic = program.block(block.id).unwrap();
    let renderer = program.render_index(&stage.ir);
    let condition = renderer
        .render_expression(semantic.condition.unwrap(), |_| true)
        .unwrap();
    let target = renderer
        .render_expression(semantic.target.unwrap(), |_| true)
        .unwrap();
    let (ctx, mut workspace) = workspace(artifact.clone());
    let high = code(&frame(&ctx, &mut workspace, vec![]));
    assert!(high.contains(&format!("if ({condition})")), "{high}");
    assert!(high.contains(&format!("jump {target} via []")), "{high}");
    let raw = code(&click(&ctx, &mut workspace, "Structured code"));
    assert!(
        raw.contains(&format!("if (v{} != 0)", raw_condition.0)),
        "{raw}"
    );
    assert!(
        raw.contains(&format!("jump v{} via []", raw_target.0)),
        "{raw}"
    );
    for text in [&high, &raw] {
        let true_arm = text.split("} else {").next().unwrap();
        assert!(true_arm.contains(qualifier), "{text}");
        assert!(
            !true_arm.contains("evm.stop()"),
            "true arm must keep its boundary: {text}"
        );
        assert_eq!(text.matches("evm.stop()").count(), 1, "{text}");
        assert_eq!(
            text.matches("JUMPI false continuation: end of code")
                .count(),
            1,
            "{text}"
        );
    }
}

#[test]
fn lifted_final_unknown_conditional_keeps_true_partial_exit_and_false_stop_in_both_routes() {
    assert_final_branch("5f3560203557", "unresolved_target (partial)");
}

#[test]
fn lifted_final_invalid_conditional_keeps_true_exception_and_false_stop_in_both_routes() {
    assert_final_branch("5f3560ff57", "exceptional_invalid_target");
}

#[test]
fn high_transfer_rendering_uses_model_liveness_for_known_return_address_bindings() {
    let artifact = artifact("60056007565b005b56");
    let current = artifact.current().unwrap();
    let program = current.ir.lifted.as_ref().unwrap();
    let removed: Vec<_> = program
        .blocks
        .iter()
        .flat_map(|block| {
            current
                .ir
                .block(block.block)
                .unwrap()
                .parameters
                .iter()
                .filter(|parameter| !block.parameters.contains(parameter))
                .copied()
        })
        .collect();
    assert!(
        !removed.is_empty(),
        "fixture must carry a raw return address that is absent from high parameters"
    );
    let transfers: Vec<_> = program
        .transfers
        .iter()
        .filter_map(|transfer| program.render_transfer(transfer, &current.ir))
        .collect();
    let (ctx, mut workspace) = workspace(artifact);
    let high = code(&frame(&ctx, &mut workspace, vec![]));
    for parameter in &removed {
        assert!(!high.contains(&format!("v{}", parameter.0)), "{high}");
    }
    for transfer in transfers {
        assert!(high.contains(&transfer), "{high}");
    }
    let raw = code(&click(&ctx, &mut workspace, "Structured code"));
    assert!(
        removed
            .iter()
            .any(|parameter| raw.contains(&format!("v{}", parameter.0))),
        "{raw}"
    );
}

#[test]
fn late_semantic_statement_and_expression_navigation_use_virtual_entity_rows() {
    let bytecode = (0..250)
        .map(|offset| format!("61{offset:04x}3550"))
        .chain(std::iter::once("00".into()))
        .collect::<String>();
    let artifact = artifact(&bytecode);
    let current = artifact.current().unwrap();
    let program = current.ir.lifted.as_ref().unwrap();
    let statement = EntityRef {
        stage: current.id,
        entity: program
            .blocks
            .last()
            .unwrap()
            .statements
            .iter()
            .rev()
            .find(|statement| {
                current
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(|op| op.id == statement.operation && op.opcode == 0x35)
            })
            .unwrap()
            .id,
    };
    let expression = EntityRef {
        stage: current.id,
        entity: program.expressions.last().unwrap().id,
    };
    let (ctx, mut workspace) = workspace(artifact.clone());
    for (at, route, role) in [
        (statement, "Simplified structured code", "decompile-code"),
        (expression, "IR", "decompile-ir"),
    ] {
        assert!(workspace.state.navigate(&artifact, at));
        let visible = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), route);
        assert!(
            visible
                .iter()
                .any(|row| row["role"] == role && row["selected"] == true)
        );
        assert!(!workspace.state.entity_navigation);
        assert!(crate::decompile::rows::prepared(&ctx) < 100);
        assert_eq!(
            workspace
                .state
                .index
                .stage(at.stage)
                .containing_block(at.entity),
            Some(0)
        );
    }
}

#[test]
fn model_admission_accepts_legacy_raw_and_rejects_mislabeled_or_corrupt_lifted_entities() {
    let mut legacy = fixture::artifact(evm_abstract_protocol::DecompileStrategy::Dispatcher);
    legacy.schema_version = 1;
    model::check_artifact(&legacy).unwrap();
    let mut high = artifact(BOOLEAN);
    let (ctx, mut workspace) = workspace(legacy);
    assert_eq!(workspace.views.route_label(), "Structured code");
    // This real ordinary-branch projection contains no guard metadata and can
    // represent a schema-2 producer. Keep legacy high-view coverage explicitly;
    // certified guards are never stripped merely to recover old expectations.
    let mut legacy_high = artifact("5f35600a576001600d565b60025b5f5500");
    assert!(legacy_high.stages.iter().all(|stage| {
        stage
            .ir
            .lifted
            .as_ref()
            .is_none_or(|program| !program.has_guards())
    }));
    legacy_high.schema_version = 2;
    legacy_high.readability.clear();
    model::check_artifact(&legacy_high).unwrap();
    deliver(&mut workspace, legacy_high, 11);
    let legacy_code = code(&frame(&ctx, &mut workspace, vec![]));
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(legacy_code.contains("if (") && legacy_code.contains("evm.sstore("));
    assert!(!legacy_code.contains("require("));
    deliver(&mut workspace, high.clone(), 12);
    frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    for old_version in [1, 2] {
        let mut old = high.clone();
        old.schema_version = old_version;
        old.readability.clear();
        assert!(model::check_artifact(&old).is_err());
    }
    high.schema_version = evm_abstract_protocol::decompile_model::SCHEMA_VERSION;
    let current = high.current_stage.unwrap();
    let stage = high
        .stages
        .iter_mut()
        .find(|stage| stage.id == current)
        .unwrap();
    stage.ir.lifted.as_mut().unwrap().expressions[0].id = stage.ir.blocks[0].id;
    assert!(model::check_artifact(&high).is_err());
}
