//! Display preferences are checked against actual published semantic witnesses.
use super::{Workspace, deliver, model};
use crate::{controls, decompile::structured};
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    DecompileArtifact, DecompileRequest, JobState,
    decompile_model::{EntityId, EntityRef, LiftedRenderError, Stage, StageKind, StructuredKind},
};
use evm_abstract_server::jobs::{Config, Pool};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

mod annotations;

// A calldata branch joins a live word after MSTORE/MLOAD/SSTORE/SLOAD. The
// joined word is stored, then CALL, LOG0 and RETURN remain observable.
const EFFECTS_BRANCH: &str = concat!(
    "602a5f525f51805f555f54015f35601757",
    "600101601b565b6002015b805f525f55",
    "5f5f5f5f5f600160fff15060205fa060205ff3",
);
// A live counter crosses the loop header, body backedge and storage-writing exit.
const LOOP: &str = "5f5b8060039010600e575b5f55005b600101600156";
// Two entries into a storage-incrementing SCC require a real dispatcher.
const DISPATCHER: &str = concat!(
    "5f356018575b5f54600101805f5560049010601857602b56",
    "5b5f54600101805f5560049010600557602b565b00",
);

fn artifact(bytecode: &str) -> DecompileArtifact {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 1,
    })
    .unwrap();
    let job = pool
        .submit_decompile(DecompileRequest {
            bytecode: bytecode.into(),
            ..DecompileRequest::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = pool.decompile_status(job.id).unwrap();
        if snapshot.state == JobState::Completed {
            break;
        }
        assert!(Instant::now() < deadline, "{snapshot:?}");
        thread::sleep(Duration::from_millis(2));
    }
    let artifact = pool
        .decompile_result(job.id)
        .unwrap()
        .artifact
        .as_ref()
        .clone();
    pool.shutdown().unwrap();
    model::check_artifact(&artifact).unwrap();
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    artifact
}

fn high(stage: &Stage, dependencies: bool) -> Vec<structured::Line> {
    structured::lifted_lines_indexed(
        stage,
        &crate::decompile::index::StageIndex::new(&stage.ir),
        structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            ..structured::HighDisplayOptions::default()
        },
    )
}

fn rows(lines: &[structured::Line]) -> Vec<(EntityId, usize, String)> {
    lines
        .iter()
        .map(|line| (line.entity, line.indent, line.text.clone()))
        .collect()
}

fn without_annotations(text: &str) -> &str {
    text.split_once(" // memory")
        .or_else(|| text.split_once(" // effect"))
        .map_or(text, |(semantic, _)| semantic)
}

fn copied_text(lines: &[structured::Line]) -> String {
    lines
        .iter()
        .map(|line| format!("{}{}", "    ".repeat(line.indent), line.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_only_dependency_metadata_changes(stage: &Stage) {
    let raw = rows(&structured::lines(stage));
    let normal = high(stage, false);
    let debug = high(stage, true);
    let program = stage.ir.lifted.as_ref().unwrap();
    // Build the only removable rows from edge witnesses, rather than treating
    // every assignment with a particular prefix as disposable metadata.
    let tokens: BTreeSet<_> = program
        .transfers
        .iter()
        .flat_map(|transfer| {
            let edge = stage
                .ir
                .edges
                .iter()
                .find(|edge| edge.id == transfer.edge)
                .unwrap();
            let target = stage.ir.block(edge.to).unwrap();
            [
                (
                    transfer.id,
                    format!(
                        "m{} = m{}; // memory token on e{}",
                        target.memory_parameter.0, edge.memory.0, edge.id.0
                    ),
                ),
                (
                    transfer.id,
                    format!(
                        "q{} = q{}; // effect token on e{}",
                        target.effect_parameter.0, edge.effect.0, edge.id.0
                    ),
                ),
            ]
        })
        .collect();
    let expected: Vec<_> = debug
        .iter()
        .filter(|line| !tokens.contains(&(line.entity, line.text.clone())))
        .map(|line| {
            (
                line.entity,
                line.indent,
                without_annotations(&line.text).to_owned(),
            )
        })
        .collect();
    // Removing dependency rows may leave an else with no body after copy
    // propagation. PR161 omits that separator; all actual rows must still match
    // exactly, including their owners and indentation. Only adjacent braces of
    // the same control owner qualify, never a nonempty arm or nested control.
    let expected: Vec<_> = expected
        .iter()
        .enumerate()
        .filter(|(position, row)| {
            row.2 != "} else {"
                || !expected
                    .get(position + 1)
                    .is_some_and(|next| next.0 == row.0 && next.1 == row.1 && next.2 == "}")
        })
        .map(|(_, row)| row.clone())
        .collect();
    assert_eq!(
        rows(&normal),
        expected,
        "semantic rows, order, entity IDs and indentation must survive"
    );
    if !stage.ir.edges.is_empty() {
        assert!(
            debug.len() > normal.len(),
            "exercise standalone dependency rows"
        );
    }
    for transfer in &program.transfers {
        if let Some(text) = program.render_transfer(transfer, &stage.ir) {
            for lines in [&normal, &debug] {
                assert!(
                    lines
                        .iter()
                        .any(|line| line.entity == transfer.id && line.text == text),
                    "live word assignment {text} is missing"
                );
            }
        }
    }
    assert_eq!(rows(&structured::lines(stage)), raw);
}

fn assert_primitive_retained(stage: &Stage, opcode: u8) {
    let op = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == opcode)
        .unwrap();
    let effects = &op.effects;
    assert!(match opcode {
        0x51 => effects.memory_read,
        0x52 => effects.memory_write,
        0x54 => effects.storage_read,
        0x55 => effects.storage_write,
        0xa0 => effects.log,
        0xf1 => effects.call && effects.world_write,
        _ => panic!("unexpected fixture primitive"),
    });
    let program = stage.ir.lifted.as_ref().unwrap();
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == op.id)
        .unwrap();
    let detailed = program.render_statement(statement, &stage.ir);
    let text = without_annotations(&detailed);
    assert!(
        text.contains(&format!("{}(", op.name.to_lowercase())),
        "{text}"
    );
    for output in &statement.outputs {
        assert!(
            text.split_once(" = ")
                .unwrap()
                .0
                .contains(&format!("v{}", output.0)),
            "{text}"
        );
    }
    for dependencies in [false, true] {
        let expected = if dependencies {
            detailed.as_str()
        } else {
            text
        };
        assert!(
            high(stage, dependencies)
                .iter()
                .any(|line| line.entity == statement.id && line.text == expected),
            "primitive {} lost in dependency mode {dependencies}",
            op.name
        );
    }
}

#[test]
fn high_rows_keep_effects_live_words_and_control_in_branches_loops_and_dispatchers() {
    for (bytecode, expected_kind, required) in [
        (
            EFFECTS_BRANCH,
            "branch",
            &[0x51, 0x52, 0x54, 0x55, 0xa0, 0xf1][..],
        ),
        (LOOP, "loop", &[0x55][..]),
        (DISPATCHER, "dispatcher", &[0x54, 0x55][..]),
    ] {
        let artifact = artifact(bytecode);
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let mut kinds = BTreeSet::new();
        structured::visit(
            &stage.ir.structured.as_ref().unwrap().root,
            &mut |node| match node.kind {
                StructuredKind::If { .. } => {
                    kinds.insert("branch");
                }
                StructuredKind::While { .. } => {
                    kinds.insert("loop");
                }
                StructuredKind::Dispatcher { .. } => {
                    kinds.insert("dispatcher");
                }
                _ => {}
            },
        );
        assert!(
            kinds.contains(expected_kind),
            "fixture did not exercise {expected_kind}"
        );
        assert_only_dependency_metadata_changes(stage);
        for opcode in required {
            assert_primitive_retained(stage, *opcode);
        }
        if expected_kind != "dispatcher" {
            assert!(
                stage
                    .ir
                    .lifted
                    .as_ref()
                    .unwrap()
                    .transfers
                    .iter()
                    .any(|transfer| !transfer.arguments.is_empty()),
                "fixture needs live word transfers"
            );
        }
        if expected_kind == "branch" {
            for dependencies in [false, true] {
                assert!(
                    high(stage, dependencies)
                        .iter()
                        .any(|line| line.text.starts_with("evm.return("))
                );
            }
        }
        assert_eq!(
            artifact, original,
            "display options must preserve IR and provenance"
        );
    }
}

#[test]
fn dependency_visibility_preserves_unknown_invalid_and_fault_boundaries() {
    for (bytecode, required) in [
        ("5f3560010156", "unresolved_target (partial)"),
        ("5f3560ff57", "exceptional_invalid_target"),
        ("602a5f5255", "not completed"),
    ] {
        let artifact = artifact(bytecode);
        let stage = artifact.current().unwrap();
        assert_only_dependency_metadata_changes(stage);
        for dependencies in [false, true] {
            let lines = high(stage, dependencies);
            assert!(
                lines.iter().any(|line| line.text.contains(required)),
                "{bytecode}: {}",
                copied_text(&lines)
            );
            if bytecode == "602a5f5255" {
                assert_primitive_retained(stage, 0x52);
                assert!(
                    lines
                        .iter()
                        .any(|line| line.text.contains("fault") && line.text.contains("SSTORE"))
                );
                assert!(
                    !lines.iter().any(|line| line.text.starts_with("sstore(")),
                    "faulting SSTORE must not appear completed"
                );
            }
        }
    }
}

#[test]
fn shared_formatter_keeps_full_detail_defaults_work_limits_and_errors() {
    let artifact = artifact(EFFECTS_BRANCH);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let renderer = program.render_index(&stage.ir);
    let mut annotated = 0;
    for statement in program.blocks.iter().flat_map(|block| &block.statements) {
        let mut default_work = 0;
        let full = renderer
            .render_statement(statement, |work| {
                default_work += work;
                true
            })
            .unwrap();
        assert_eq!(program.render_statement(statement, &stage.ir), full);
        assert_eq!(
            crate::decompile::lifted::statement(&renderer, statement),
            full
        );
        for dependencies in [false, true] {
            let mut work_used = 0;
            let text = renderer
                .render_statement_with_dependencies(statement, dependencies, |work| {
                    work_used += work;
                    true
                })
                .unwrap();
            assert_eq!(
                text,
                if dependencies {
                    full.as_str()
                } else {
                    without_annotations(&full)
                }
            );
            assert_eq!(work_used, default_work);
            if !statement.inputs.is_empty() {
                assert_eq!(
                    renderer.render_statement_with_dependencies(statement, dependencies, |_| false),
                    Err(LiftedRenderError::Limit)
                );
            }
        }
        annotated += usize::from(without_annotations(&full) != full);
        let mut missing = statement.clone();
        missing.operation = EntityId(u64::MAX);
        for dependencies in [false, true] {
            assert_eq!(
                renderer.render_statement_with_dependencies(&missing, dependencies, |_| true),
                Err(LiftedRenderError::MissingWitness(missing.operation))
            );
        }
    }
    assert!(annotated >= 6, "exercise actual state-dependent primitives");
}

struct Frame {
    controls: Vec<Value>,
    copied: Vec<String>,
}

fn frame(ctx: &Context, workspace: &mut Workspace, events: Vec<Event>) -> Frame {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1800.0, 2400.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            controls::begin_observation(ui.ctx(), true);
            assert!(
                workspace.show(ui).is_none(),
                "display controls must not submit tasks"
            );
        },
    );
    output.textures_delta.clear();
    Frame {
        controls: serde_json::from_str(&controls::observed(ctx).unwrap()).unwrap(),
        copied: output
            .platform_output
            .commands
            .into_iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .collect(),
    }
}

fn control<'a>(frame: &'a Frame, label: &str) -> &'a Value {
    frame
        .controls
        .iter()
        .find(|control| control["label"] == label)
        .unwrap_or_else(|| panic!("missing {label}: {:?}", frame.controls))
}

fn center(control: &Value) -> Pos2 {
    let [left, top, right, bottom]: [f32; 4] =
        serde_json::from_value(control["rect"].clone()).unwrap();
    Pos2::new((left + right) * 0.5, (top + bottom) * 0.5)
}

fn click(ctx: &Context, workspace: &mut Workspace, label: &str) -> Frame {
    let visible = frame(ctx, workspace, vec![]);
    let pos = center(control(&visible, label));
    frame(
        ctx,
        workspace,
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        workspace,
        vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    )
}

fn setup(artifact: DecompileArtifact) -> (Context, Workspace) {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = Workspace::default();
    workspace.form.open = false;
    workspace.inspector = false;
    deliver(&mut workspace, artifact, 10);
    frame(&ctx, &mut workspace, vec![]);
    (ctx, workspace)
}

fn labels(frame: &Frame, role: &str) -> Vec<String> {
    frame
        .controls
        .iter()
        .filter(|control| control["role"] == role)
        .map(|control| control["label"].as_str().unwrap().to_owned())
        .collect()
}

fn assert_painted(frame: &Frame, lines: &[structured::Line]) {
    let painted = labels(frame, "decompile-code");
    assert_eq!(
        painted.len(),
        lines.len(),
        "fixture must fit the native viewport"
    );
    for (label, line) in painted.iter().zip(lines) {
        assert!(
            label.ends_with(&format!("{}{}", "    ".repeat(line.indent), line.text)),
            "{label} != {}",
            line.text
        );
    }
}

#[test]
fn actual_toggle_copy_routes_and_publications_share_the_current_display_option() {
    let artifact = artifact(EFFECTS_BRANCH);
    let stage = artifact.current().unwrap();
    let normal = high(stage, false);
    let debug = high(stage, true);
    let raw = structured::lines(stage);
    let statement = stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| {
            stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|op| op.id == statement.operation && op.opcode == 0x52)
        })
        .unwrap();
    let at = EntityRef {
        stage: stage.id,
        entity: statement.id,
    };
    let source = artifact.origin(at).unwrap().source[0].pc;
    let (ctx, mut workspace) = setup(artifact.clone());
    let stages = (workspace.state.before, workspace.state.current);
    let initial = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert_eq!(
        control(&initial, "Show state dependencies")["role"],
        "toggle"
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

    click(&ctx, &mut workspace, "Transform inspector");
    workspace.state.select(&artifact, at);
    let inspector_before = labels(
        &frame(&ctx, &mut workspace, vec![]),
        "decompile-entity-detail",
    );
    assert!(
        inspector_before
            .iter()
            .any(|line| line.contains(" // memory") && line.contains(" // effect"))
    );
    let changed = click(&ctx, &mut workspace, "Show state dependencies");
    assert_painted(&changed, &debug);
    let debug_frame = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        control(&debug_frame, "Show state dependencies")["selected"],
        true
    );
    assert_eq!(
        labels(&debug_frame, "decompile-entity-detail"),
        inspector_before
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&debug)]
    );

    let raw_frame = click(&ctx, &mut workspace, "Structured code");
    assert!(
        !raw_frame
            .controls
            .iter()
            .any(|control| control["label"] == "Show state dependencies")
    );
    assert_painted(&raw_frame, &raw);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&raw)]
    );
    let ir_before = labels(&click(&ctx, &mut workspace, "IR"), "decompile-ir");
    assert!(!ir_before.is_empty());
    assert_painted(
        &click(&ctx, &mut workspace, "Simplified structured code"),
        &debug,
    );
    assert_painted(
        &click(&ctx, &mut workspace, "Show state dependencies"),
        &normal,
    );
    assert_eq!(
        labels(&click(&ctx, &mut workspace, "IR"), "decompile-ir"),
        ir_before
    );
    assert_painted(&click(&ctx, &mut workspace, "Structured code"), &raw);
    click(&ctx, &mut workspace, "Simplified structured code");
    click(&ctx, &mut workspace, "Show state dependencies");
    assert_eq!((workspace.state.before, workspace.state.current), stages);
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));

    let old_index = workspace.state.index.stage(stage.id);
    let old_details = workspace.state.details(&artifact, at);
    let old_sources = workspace.state.source_descendants(&artifact, source);
    deliver(&mut workspace, artifact.clone(), 11);
    assert!(!Rc::ptr_eq(
        &old_index,
        &workspace.state.index.stage(stage.id)
    ));
    assert!(!Rc::ptr_eq(
        &old_details,
        &workspace.state.details(&artifact, at)
    ));
    assert!(!Rc::ptr_eq(
        &old_sources,
        &workspace.state.source_descendants(&artifact, source)
    ));
    let installed = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        control(&installed, "Show state dependencies")["selected"],
        true
    );
    assert_painted(&installed, &debug);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&debug)]
    );
    assert_eq!(workspace.state.publication.unwrap().job.0, 11);
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
}

fn follow_source_link(
    ctx: &Context,
    workspace: &mut Workspace,
    artifact: &DecompileArtifact,
    at: EntityRef,
) -> Frame {
    let source = artifact.origin(at).unwrap().source[0].pc;
    assert!(
        workspace
            .state
            .source_descendants(artifact, source)
            .contains(&at)
    );
    workspace.inspector = true;
    workspace.state.select_source(artifact, source);
    let label = format!(
        "Source descendant s{}/e{} · Some(LiftedTransfer)",
        at.stage.0, at.entity.0
    );
    let mut seen = Vec::new();
    // Follow the real inspector link, scrolling its virtual descendant list.
    for _ in 0..24 {
        let visible = frame(ctx, workspace, vec![]);
        seen = labels(&visible, "decompile-entity-link");
        if visible
            .controls
            .iter()
            .any(|control| control["label"] == label)
        {
            return click(ctx, workspace, &label);
        }
        let last = visible
            .controls
            .iter()
            .rev()
            .find(|control| {
                control["role"] == "decompile-entity-link"
                    && control["label"]
                        .as_str()
                        .unwrap()
                        .starts_with("Source descendant ")
            })
            .expect("visible source descendant links");
        frame(
            ctx,
            workspace,
            vec![
                Event::PointerMoved(center(last)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -80.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    panic!("source link {label} never became visible: {seen:?}");
}

#[test]
fn source_links_fall_back_to_ir_for_hidden_empty_transfers_and_keep_live_word_code_anchors() {
    for (bytecode, empty) in [("60056007565b005b56", true), (EFFECTS_BRANCH, false)] {
        let artifact = artifact(bytecode);
        let stage = artifact.current().unwrap();
        let transfer = stage
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .transfers
            .iter()
            .find(|transfer| transfer.arguments.is_empty() == empty)
            .unwrap();
        let at = EntityRef {
            stage: stage.id,
            entity: transfer.id,
        };
        assert_eq!(
            high(stage, false)
                .iter()
                .any(|line| line.entity == at.entity),
            !empty
        );
        let (ctx, mut workspace) = setup(artifact.clone());
        let navigated = follow_source_link(&ctx, &mut workspace, &artifact, at);
        assert_eq!(workspace.state.selected.as_ref().unwrap().at, at);
        assert_eq!(
            workspace.views.route_label(),
            if empty {
                "IR"
            } else {
                "Simplified structured code"
            }
        );
        let role = if empty {
            "decompile-ir"
        } else {
            "decompile-code"
        };
        assert!(
            navigated
                .controls
                .iter()
                .any(|control| control["role"] == role && control["selected"] == true)
        );
        click(&ctx, &mut workspace, "Simplified structured code");
        assert_eq!(
            workspace.views.route_label(),
            "Simplified structured code",
            "explicit route survives a navigation fallback"
        );
        click(&ctx, &mut workspace, "Show state dependencies");
        let navigated = follow_source_link(&ctx, &mut workspace, &artifact, at);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        let selected = labels(
            &Frame {
                controls: navigated
                    .controls
                    .into_iter()
                    .filter(|control| control["selected"] == true)
                    .collect(),
                copied: Vec::new(),
            },
            "decompile-code",
        );
        if empty {
            assert!(selected.iter().any(|line| line.contains("memory token")));
            assert!(selected.iter().any(|line| line.contains("effect token")));
        } else {
            let tuple = stage
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .render_transfer(transfer, &stage.ir)
                .unwrap();
            assert!(selected.iter().any(|line| line.ends_with(&tuple)));
        }
        assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
    }
}
