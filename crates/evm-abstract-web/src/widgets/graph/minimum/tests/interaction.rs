//! Real egui pointer/key input over an explicitly pending/historical display
//! collision built from retained producer receipts. No new closure is certified.
use super::super::control::Destination;
use super::{Graph, ReportViewId, Selection};
use crate::widgets::graph::GraphIntent;
use egui::{
    Context, Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2,
};
use evm_abstract_protocol::{
    AnalysisInput, AnalysisNamespace, AnalysisReport, AnalyzeRequest, BytecodeInput,
    ProcessLocation,
};

struct Frame {
    output: FullOutput,
    canvas: Rect,
    ui_id: Id,
    intent: Option<GraphIntent>,
}

fn draw(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
    events: Vec<Event>,
) -> Frame {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
        events,
        ..Default::default()
    };
    graph.prepare_input(&mut input);
    let mut canvas = Rect::NOTHING;
    let mut ui_id = Id::NULL;
    let mut intent = None;
    let mut output = ctx.run_ui(input, |ui| {
        ui_id = ui.id();
        let available = ui.available_rect_before_wrap();
        intent = graph.show_minimum_scoped(
            ui,
            report,
            ReportViewId::fixture(1, AnalysisNamespace::Baseline).minimum(None),
            selection,
        );
        let size = graph.viewport.unwrap_or(Vec2::ZERO);
        canvas = Rect::from_min_size(available.right_bottom() - size, size);
    });
    output.textures_delta.clear();
    Frame {
        output,
        canvas,
        ui_id,
        intent,
    }
}

fn click(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
    at: Pos2,
) -> Frame {
    draw(
        ctx,
        graph,
        report,
        selection,
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(
        ctx,
        graph,
        report,
        selection,
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ],
    )
}

fn label(output: &FullOutput, prefix: &str) -> Pos2 {
    output
        .shapes
        .iter()
        .find_map(|item| match &item.shape {
            egui::Shape::Text(text) if text.galley.text().starts_with(prefix) => {
                Some(text.pos + Vec2::splat(5.0))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing choice {prefix}"))
}

fn settle_choice(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
    mut frame: Frame,
    prefix: &str,
) -> Frame {
    // egui measures a newly opened Window before its rows receive their final
    // viewport. Wait for the actual choice's paint, never for elapsed time.
    for _ in 0..8 {
        if frame.output.shapes.iter().any(|item| matches!(&item.shape, egui::Shape::Text(text) if text.galley.text().starts_with(prefix))) { return frame; }
        frame = draw(ctx, graph, report, selection, Vec::new());
    }
    let labels: Vec<_> = frame
        .output
        .shapes
        .iter()
        .filter_map(|item| match &item.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect();
    let picker = graph
        .scene
        .minimum
        .as_ref()
        .and_then(|scene| scene.picker)
        .map(|picker| match picker {
            super::super::scene::Picker::Origins(id) => format!("origins {id}"),
            super::super::scene::Picker::Targets(id) => format!("targets {id}"),
        });
    panic!("choice {prefix} did not become visible; picker {picker:?}; labels {labels:?}");
}

fn pending_historical() -> (AnalysisReport, usize) {
    let mut request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "36565b36565b00".into(),
            address: None,
        }),
        ..Default::default()
    };
    request.limits.root_terminal_coverage = false;
    let mut report = evm_abstract_server::analyze::analyze(request).unwrap();
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let summary = crate::app::minimum::initial_summary(model);
    let hub = model.summaries[summary]
        .relation
        .regional_cfg
        .nodes
        .iter_mut()
        .find(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == ProcessLocation::JumpTargets)
        })
        .unwrap();
    hub.transfer = None;
    let later = model.summaries[summary].clone();
    model.summaries.push(later);
    (report, summary)
}

#[test]
fn pending_hub_can_select_each_exact_receipt_and_open_every_decoded_target_separately() {
    let (report, summary) = pending_historical();
    let mut selection = Selection::default();
    selection.select_summary(report.minimum.as_deref().unwrap(), summary);
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    let first = draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
    let scene = graph.scene.minimum.as_ref().unwrap();
    let id = scene
        .control
        .cards
        .iter()
        .position(|card| {
            matches!(
                card.kind,
                super::super::control::CardKind::Source {
                    location: ProcessLocation::JumpTargets,
                    ..
                }
            )
        })
        .unwrap();
    let card = &scene.control.cards[id];
    assert!(
        card.origins.len() > 1,
        "fixture must preserve grouped independent origins"
    );
    assert!(card.origins.iter().all(|origin| !origin.current));
    let origins: Vec<_> = card.origins.iter().map(|origin| origin.target).collect();
    let destinations = scene.control.target_sets[card.target_set.unwrap()]
        .entries
        .clone();
    assert_eq!(destinations.len(), 2);
    let hub = scene.placement.nodes[&id];
    graph.restore_camera(
        1.0,
        Vec2::new(
            first.canvas.width() * 0.5 - hub.center().x,
            24.0 - hub.top(),
        ),
    );
    for (index, target) in origins.iter().enumerate() {
        let frame = draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
        let rect = graph.screen_rect(frame.canvas, hub);
        let action = super::super::paint::actions(
            rect,
            &graph.scene.minimum.as_ref().unwrap().control.cards[id],
            graph.zoom,
            crate::widgets::graph::paint::DetailLevel::Preview,
        );
        let choices = click(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            if index == 0 {
                assert!(action.body.width() > 0.0);
                action.body.center()
            } else {
                action.receipts.unwrap().center()
            },
        );
        let prefix = format!(
            "r{} {}",
            target.origin.node,
            if target.origin.location.is_none() {
                "Body"
            } else {
                "MAY point"
            }
        );
        let choices = settle_choice(&ctx, &mut graph, &report, &mut selection, choices, &prefix);
        let at = label(&choices.output, &prefix);
        let picked = click(&ctx, &mut graph, &report, &mut selection, at);
        assert!(matches!(picked.intent, Some(GraphIntent::Select(actual)) if actual == *target));
        assert_eq!(selection.node, Some(target.origin.node));
        assert_eq!(selection.control, target.origin.location);
    }
    for entry in destinations {
        let frame = draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
        let rect = graph.screen_rect(frame.canvas, hub);
        let action = super::super::paint::actions(
            rect,
            &graph.scene.minimum.as_ref().unwrap().control.cards[id],
            graph.zoom,
            crate::widgets::graph::paint::DetailLevel::Preview,
        );
        let choices = click(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            action.targets.unwrap().center(),
        );
        let Destination::Source { program, location } = entry.destination else {
            panic!("pending target must remain decoded-only")
        };
        let ProcessLocation::Block(index) = location else {
            panic!("legal source destination")
        };
        let block = report
            .programs
            .iter()
            .find(|row| row.id == program)
            .unwrap()
            .blocks[index]
            .start_pc;
        let prefix = format!("Decoded source · Block 0x{block:04x}");
        let choices = settle_choice(&ctx, &mut graph, &report, &mut selection, choices, &prefix);
        let at = label(&choices.output, &prefix);
        let before = selection;
        let picked = click(&ctx, &mut graph, &report, &mut selection, at);
        assert!(
            matches!(picked.intent, Some(GraphIntent::OpenSource { program: actual, location: point, .. }) if actual == program && point == location)
        );
        assert_eq!(
            selection, before,
            "decoded target must not manufacture a regional/native selection"
        );
    }
    let frame = draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
    ctx.memory_mut(|memory| {
        memory.request_focus(frame.ui_id.with(("control_receipts", summary, id)))
    });
    let keyed = draw(
        &ctx,
        &mut graph,
        &report,
        &mut selection,
        vec![Event::Key {
            key: Key::Enter,
            physical_key: Some(Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(keyed.output.shapes.iter().any(|item| matches!(&item.shape, egui::Shape::Text(text) if text.galley.text() == "Control receipts")));
}

#[test]
fn narrow_scope_geometry_and_camera_survive_resize_and_return_from_a_different_window() {
    let code = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout/long-linear-chain-600.hex"
    ));
    let request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: code.trim().into(),
            address: None,
        }),
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    let mut selection = Selection::default();
    selection.select_summary(
        report.minimum.as_deref().unwrap(),
        crate::app::minimum::initial_summary(report.minimum.as_deref().unwrap()),
    );
    let original = selection;
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    super::frame(&ctx, &mut graph, &report, &mut selection);
    let scene = graph.scene.minimum.as_ref().unwrap();
    assert_eq!(scene.control.counts.displayed_points, 3);
    assert_eq!(scene.control.counts.total_points, 600);
    let geometry = scene.placement.nodes.clone();
    let camera = graph.camera();
    draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
    assert_eq!(
        graph.scene.minimum.as_ref().unwrap().placement.nodes,
        geometry
    );
    assert_eq!(graph.camera(), camera);
    selection.control = Some(ProcessLocation::Block(20));
    draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
    assert_eq!(
        graph
            .scene
            .minimum
            .as_ref()
            .unwrap()
            .control
            .counts
            .displayed_points,
        12
    );
    selection = original;
    graph.restore_camera(camera.0, camera.1);
    draw(&ctx, &mut graph, &report, &mut selection, Vec::new());
    let restored = graph.scene.minimum.as_ref().unwrap();
    assert_eq!(restored.control.counts.displayed_points, 3);
    assert_eq!(restored.placement.nodes, geometry);
    assert_eq!(graph.camera(), camera);
}
