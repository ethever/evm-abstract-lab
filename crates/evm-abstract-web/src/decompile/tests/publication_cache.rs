use super::{
    Artifact, Command, Completion, EntityId, JobOperation, Message, StageId, Workspace, api,
    deliver, fixture, model, start,
};
use crate::controls;
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::decompile_model::{
    EntityRef, Origin, SourceSpan, Value, ValueFact, ValueKind, records,
};
use serde_json::Value as Control;
use std::collections::{BTreeMap, BTreeSet};

const ROUTES: [&str; 5] = [
    "Structured code",
    "Graphs",
    "IR",
    "Changes",
    "Original bytecode",
];

fn frame(ctx: &Context, workspace: &mut Workspace, events: Vec<Event>) -> Vec<Control> {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
        events,
        ..RawInput::default()
    };
    workspace.prepare_input(&mut input);
    let mut output = ctx.run_ui(input, |ui| {
        controls::begin_observation(ui.ctx(), true);
        workspace.show(ui);
    });
    output.textures_delta.clear();
    serde_json::from_str(&controls::observed(ctx).unwrap()).unwrap()
}

fn rect(control: &Control) -> Rect {
    let [left, top, right, bottom]: [f32; 4] =
        serde_json::from_value(control["rect"].clone()).unwrap();
    Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom))
}

fn tab(ctx: &Context, workspace: &mut Workspace, label: &str) -> Vec<Control> {
    let controls = frame(ctx, workspace, vec![]);
    let pos = rect(
        controls
            .iter()
            .find(|row| row["label"] == label)
            .unwrap_or_else(|| panic!("missing tab {label}: {controls:?}")),
    )
    .center();
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
    assert_eq!(workspace.views.route_label(), label);
    frame(ctx, workspace, vec![])
}

fn labels(controls: &[Control], role: &str) -> Vec<String> {
    controls
        .iter()
        .filter(|row| row["role"] == role)
        .map(|row| row["label"].as_str().unwrap().to_owned())
        .collect()
}

fn snapshot(ctx: &Context, workspace: &mut Workspace) -> BTreeMap<&'static str, Vec<String>> {
    ROUTES
        .into_iter()
        .zip([
            "decompile-code",
            "graph-preview",
            "decompile-ir",
            "decompile-difference",
            "decompile-source",
        ])
        .map(|(route, role)| (route, labels(&tab(ctx, workspace, route), role)))
        .collect()
}

fn graph_blocks(controls: &[Control]) -> Vec<Rect> {
    let blocks: Vec<_> = controls
        .iter()
        .filter(|row| row["role"] == "decompile-block")
        .map(rect)
        .collect();
    assert_eq!(
        blocks.len(),
        2,
        "both comparison panes must paint their block"
    );
    blocks
}

fn graph_placement(controls: &[Control]) -> Vec<(Rect, Rect)> {
    let blocks = graph_blocks(controls);
    let canvases: Vec<_> = controls
        .iter()
        .filter(|row| row["role"] == "graph-canvas")
        .map(rect)
        .collect();
    assert_eq!(canvases.len(), blocks.len());
    canvases.into_iter().zip(blocks).collect()
}

fn assert_retained_view(before: &[(Rect, Rect)], controls: &[Control]) {
    let after = graph_placement(controls);
    assert_eq!(before.len(), after.len());
    for ((old_canvas, old_block), (canvas, block)) in before.iter().zip(after) {
        // Status/error text can resize the available content. Retain the
        // manual world-space view and zoom relative to that actual canvas.
        let old_offset = old_block.center() - old_canvas.center();
        let offset = block.center() - canvas.center();
        assert!(
            (offset - old_offset).length() < 0.01,
            "pan changed: canvas {old_canvas:?} -> {canvas:?}, block {old_block:?} -> {block:?}"
        );
        assert_eq!(block.size(), old_block.size(), "manual zoom changed");
        if *old_canvas != canvas {
            eprintln!(
                "Retained view: canvas {old_canvas:?} -> {canvas:?}; block {old_block:?} -> {block:?}; center delta {:?}; size delta {:?}",
                canvas.center() - old_canvas.center(),
                canvas.size() - old_canvas.size()
            );
        }
    }
}

fn pan_graph(ctx: &Context, workspace: &mut Workspace, graph: &[Control]) -> Vec<Rect> {
    let original = graph_blocks(graph);
    let canvas = rect(
        graph
            .iter()
            .find(|row| row["role"] == "graph-canvas")
            .unwrap(),
    );
    let from = canvas.min + Vec2::new(30.0, 30.0);
    let to = from + Vec2::new(40.0, 25.0);
    frame(ctx, workspace, vec![Event::PointerMoved(from)]);
    frame(
        ctx,
        workspace,
        vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    frame(ctx, workspace, vec![Event::PointerMoved(to)]);
    frame(
        ctx,
        workspace,
        vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let panned = graph_blocks(&frame(ctx, workspace, vec![]));
    assert_ne!(
        panned[0], original[0],
        "the graph must actually have been panned"
    );
    panned
}

fn setup() -> (Context, Workspace, Artifact) {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = Workspace::default();
    workspace.form.open = false;
    workspace.inspector = false;
    let artifact = fixture::large(3);
    model::check_artifact(&artifact).unwrap();
    deliver(&mut workspace, artifact.clone(), 10);
    frame(&ctx, &mut workspace, vec![]);
    frame(&ctx, &mut workspace, vec![]);
    (ctx, workspace, artifact)
}

fn optimized_representation(first: &Artifact) -> Artifact {
    let mut next = first.clone();
    // Removing unused pure PUSH/POP pairs preserves this program's behavior.
    // Its input/configuration namespace and all stage IDs remain the same.
    for (position, pairs) in [(1, 1), (2, 2)] {
        let stage = &mut next.stages[position];
        stage.ir.blocks[0].operations.drain(..pairs * 2);
        stage
            .ir
            .values
            .retain(|value| value.id.0 < 100_000 || value.id.0 >= 100_000 + pairs as u64);
    }
    // Decode metadata can partition the same instruction stream differently.
    // Stale source positions would index past the shortened first block.
    let decoded = &mut next.stages[0];
    let mut tail = decoded.ir.blocks[0].clone();
    tail.id = EntityId(900_000);
    tail.source = Some(SourceSpan {
        pc: 3,
        end_pc: decoded.ir.bytecode_len,
    });
    tail.memory_parameter = EntityId(900_001);
    tail.effect_parameter = EntityId(900_002);
    tail.operations = decoded.ir.blocks[0].operations.split_off(2);
    decoded.ir.blocks[0].source = Some(SourceSpan { pc: 0, end_pc: 3 });
    decoded.ir.values.extend([
        Value {
            id: tail.memory_parameter,
            kind: ValueKind::MemoryParameter { block: tail.id },
            fact: ValueFact::Unknown,
        },
        Value {
            id: tail.effect_parameter,
            kind: ValueKind::EffectParameter { block: tail.id },
            fact: ValueFact::Unknown,
        },
    ]);
    decoded.ir.blocks.push(tail);
    let entities: BTreeSet<_> = next
        .stages
        .iter()
        .flat_map(|stage| {
            records(&stage.ir).into_keys().map(|entity| EntityRef {
                stage: stage.id,
                entity,
            })
        })
        .collect();
    next.mappings
        .origins
        .retain(|origin| entities.contains(&origin.entity));
    for entity in [900_000, 900_001, 900_002] {
        next.mappings.origins.push(Origin {
            entity: EntityRef {
                stage: StageId(0),
                entity: EntityId(entity),
            },
            source: vec![],
            parents: vec![],
            introduced_by: 0,
            reason: "Alternate decoded partition in presentation fixture".into(),
        });
    }
    model::check_artifact(&next).unwrap();
    next
}

#[test]
fn accepted_publication_refreshes_painted_code_graph_layout_ir_differences_and_source_rows() {
    let (ctx, mut workspace, first) = setup();
    let initial = snapshot(&ctx, &mut workspace);
    assert!(
        initial["Structured code"]
            .iter()
            .any(|line| line.contains("v100000 ="))
    );
    assert!(initial["IR"].iter().any(|line| line.contains("o1000:")));
    assert!(!initial["Changes"].is_empty());
    assert_eq!(initial["Original bytecode"].len(), 6);
    let initial_blocks = graph_blocks(&tab(&ctx, &mut workspace, "Graphs"));
    tab(&ctx, &mut workspace, "IR");
    let next = optimized_representation(&first);
    assert_eq!(next.id, first.id);
    assert_eq!(next.input, first.input);
    assert_eq!(next.config, first.config);
    assert_eq!(
        next.stages.iter().map(|stage| stage.id).collect::<Vec<_>>(),
        first
            .stages
            .iter()
            .map(|stage| stage.id)
            .collect::<Vec<_>>()
    );

    deliver(&mut workspace, next.clone(), 11);
    assert_eq!(workspace.artifact.as_ref(), Some(&next));
    assert_eq!(workspace.views.route_label(), "IR");
    assert_eq!(
        workspace.state.selected.as_ref().unwrap().publication.job.0,
        11
    );
    let refreshed = snapshot(&ctx, &mut workspace);
    let code = refreshed["Structured code"].join("\n");
    assert!(code.contains("v100002 = 0x2;"), "{code}");
    assert!(
        !code.contains("v100000") && !code.contains("v100001"),
        "{code}"
    );
    let graph = refreshed["Graphs"].join("\n");
    assert!(!graph.contains("v100000"), "{graph}");
    assert_eq!(graph.matches("v100001 =").count(), 1, "{graph}");
    assert_eq!(graph.matches("v100002 =").count(), 2, "{graph}");
    let refreshed_blocks = graph_blocks(&tab(&ctx, &mut workspace, "Graphs"));
    for (old, new) in initial_blocks.iter().zip(&refreshed_blocks) {
        assert!(
            new.height() < old.height(),
            "canvas retained old measured cards: {old:?} -> {new:?}"
        );
    }
    let ir = refreshed["IR"].join("\n");
    assert!(!ir.contains("o1000:") && !ir.contains("o1001:"), "{ir}");
    assert_eq!(ir.matches("o1002:").count(), 1, "{ir}");
    assert_eq!(ir.matches("o1004:").count(), 2, "{ir}");
    assert!(
        refreshed["Changes"]
            .iter()
            .any(|line| line.contains("Removed") && line.contains("entity 1002"))
    );
    assert_ne!(refreshed["Changes"], initial["Changes"]);
    assert_eq!(refreshed["Original bytecode"], initial["Original bytecode"]);
}

#[test]
fn accepted_publication_with_identical_scene_drops_the_nested_canvas_pan() {
    let (ctx, mut workspace, artifact) = setup();
    let generation = deliver(&mut workspace, artifact.clone(), 11);
    let graph = tab(&ctx, &mut workspace, "Graphs");
    let fitted = graph_blocks(&graph);
    pan_graph(&ctx, &mut workspace, &graph);
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Ok(fixture::snapshot(
            11,
            api::JobState::Completed,
            Some(fixture::version(2)),
        ))),
    });
    assert!(matches!(
        workspace.task.next(1.0),
        Some(Command::Result { .. })
    ));
    workspace.receive(Message::Result {
        generation,
        id: api::JobId(11),
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id: api::JobId(11),
                publication: fixture::version(2),
                artifact,
            }),
        })),
    });
    assert_eq!(
        workspace.state.publication.unwrap().version,
        fixture::version(2)
    );
    assert_eq!(workspace.views.route_label(), "Graphs");
    assert_eq!(graph_blocks(&frame(&ctx, &mut workspace, vec![])), fitted);
}

#[test]
fn submit_workspace_switch_and_rejected_attempts_retain_the_painted_publication_and_pan() {
    let (ctx, mut workspace, first) = setup();
    let initial = snapshot(&ctx, &mut workspace);
    let graph = tab(&ctx, &mut workspace, "Graphs");
    pan_graph(&ctx, &mut workspace, &graph);
    let panned = graph_placement(&frame(&ctx, &mut workspace, vec![]));
    start(&mut workspace);
    assert_retained_view(&panned, &frame(&ctx, &mut workspace, vec![]));

    // Render the independent analysis workspace, then return to this instance.
    let mut analysis = crate::Workspace::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        analysis.show(ui);
    });
    output.textures_delta.clear();
    assert_eq!(workspace.views.route_label(), "Graphs");
    assert_retained_view(&panned, &frame(&ctx, &mut workspace, vec![]));
    let selection = workspace.state.selected.clone();
    let mut invalid = first.clone();
    invalid.current_stage = Some(StageId(999));
    assert!(model::check_artifact(&invalid).is_err());
    deliver(&mut workspace, invalid, 12);
    assert!(matches!(
        workspace.task.error,
        Some(super::task::Error::Artifact(_))
    ));
    assert_retained_view(&panned, &frame(&ctx, &mut workspace, vec![]));
    assert_eq!(snapshot(&ctx, &mut workspace), initial);

    for (job, completion) in [
        (13, Completion::Failed),
        (14, Completion::Cancelled),
        (15, Completion::Limited),
    ] {
        tab(&ctx, &mut workspace, "Graphs");
        let mut unvalidated = first.clone();
        unvalidated.completion = completion;
        unvalidated.stages.clear();
        unvalidated.events.clear();
        unvalidated.mappings.origins.clear();
        unvalidated.current_stage = None;
        model::check_artifact(&unvalidated).unwrap();
        deliver(&mut workspace, unvalidated, job);
        assert!(matches!(
            workspace.task.error,
            Some(super::task::Error::UnvalidatedAttempt { .. })
        ));
        assert_eq!(workspace.artifact.as_ref(), Some(&first));
        assert_eq!(workspace.state.selected, selection);
        assert_eq!(workspace.views.route_label(), "Graphs");
        assert_retained_view(&panned, &frame(&ctx, &mut workspace, vec![]));
        assert_eq!(snapshot(&ctx, &mut workspace), initial);
    }
}
