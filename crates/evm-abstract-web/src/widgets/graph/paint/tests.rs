use std::collections::BTreeMap;

use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::AnalysisReport;

use super::super::{Graph, content::NodeView, layout::Group, projection::GraphMode};
use crate::app::Selection;

/// Render the production graph with all nodes visible. The generous viewport
/// isolates semantic zoom from clipping, and leaves layout entirely untouched.
fn frame(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
    events: Vec<Event>,
) -> (FullOutput, Rect) {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(4000.0))),
        events,
        ..RawInput::default()
    };
    graph.prepare_input(&mut input);
    let mut canvas = Rect::NOTHING;
    let mut output = ctx.run_ui(input, |ui| {
        let available = ui.available_rect_before_wrap();
        graph.show(ui, report, selection);
        canvas = Rect::from_min_size(
            available.right_bottom() - graph.viewport.unwrap(),
            graph.viewport.unwrap(),
        );
    });
    output.textures_delta.clear();
    (output, canvas)
}

fn text(output: &FullOutput) -> Vec<&str> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect()
}

fn card_text<'a>(output: &'a FullOutput, graph: &Graph, canvas: Rect) -> Vec<&'a str> {
    output
        .shapes
        .iter()
        .filter(|shape| {
            graph
                .placement
                .nodes
                .values()
                .any(|node| graph.screen_rect(canvas, *node) == shape.clip_rect)
        })
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct Geometry {
    nodes: BTreeMap<usize, Rect>,
    edges: Vec<(usize, Vec<Pos2>, Rect)>,
    groups: Vec<Group>,
    bounds: Rect,
    sizes: BTreeMap<usize, Vec2>,
}

fn geometry(graph: &Graph) -> Geometry {
    Geometry {
        nodes: graph.placement.nodes.clone(),
        edges: graph
            .placement
            .edges
            .iter()
            .map(|(id, route)| (*id, route.path.clone(), route.label))
            .collect(),
        groups: graph.placement.groups.clone(),
        bounds: graph.placement.bounds,
        sizes: graph
            .nodes
            .iter()
            .map(|(id, text)| (*id, text.size))
            .collect(),
    }
}

fn initialized(mode: GraphMode) -> (Context, Graph, AnalysisReport, Selection) {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    graph.set_mode(mode);
    let report = crate::tests::report();
    let mut selection = Selection::default();
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2();
    (ctx, graph, report, selection)
}

#[test]
fn semantic_zoom_paints_native_ids_then_exact_ssa_without_changing_geometry() {
    let (ctx, mut graph, mut report, mut selection) = initialized(GraphMode::States);
    // An ID independent of vector indices proves the overview uses native IDs.
    report.cfg[2].id = 205;
    report.ssa.blocks[2].state = 205;
    for edge in &mut report.edges {
        if edge.to == 2 {
            edge.to = 205;
        }
    }
    graph.reset_report();
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    let measured = geometry(&graph);
    for (zoom, preview) in [
        (0.20, false),
        (0.54, false),
        (0.60, false),
        (0.66, true),
        (0.60, true),
        (0.54, false),
        (1.0, true),
    ] {
        graph.zoom = zoom;
        graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * zoom;
        let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let labels = card_text(&output, &graph, canvas);
        if preview {
            assert!(labels.contains(&"σ₂₀₅  ·  B₂"));
            assert!(labels.iter().any(|line| line.contains("%5 = ADD %1, %2")));
        } else {
            assert!(
                labels.contains(&"σ₂₀₅"),
                "missing native identity at zoom {zoom}"
            );
            assert!(labels.contains(&"B₂"));
            assert!(labels.iter().all(|line| !line.contains('%')));
            assert!(!labels.contains(&"σ₂₀₅  ·  B₂"));
        }
        assert_eq!(geometry(&graph), measured, "zoom {zoom} changed layout");
    }
}

#[test]
fn semantic_zoom_hysteresis_preserves_painted_detail_around_the_threshold() {
    let (ctx, mut graph, report, mut selection) = initialized(GraphMode::States);
    for (zoom, detailed) in [
        (0.54, false),
        (0.60, false),
        (0.649, false),
        (0.65, true),
        (0.60, true),
        (0.551, true),
        (0.55, false),
    ] {
        graph.zoom = zoom;
        let (output, _) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        assert_eq!(
            text(&output).iter().any(|line| line.contains("%5 = ADD")),
            detailed,
            "unexpected painted detail at zoom {zoom}"
        );
    }
}

#[test]
fn semantic_zoom_preserves_real_spatial_group_rectangles_and_members() {
    let (ctx, mut graph, mut report, mut selection) = initialized(GraphMode::States);
    // Removing the shortcut leaves a strict 0 -> 1 -> 2 sequence. This must
    // exercise a real container, rather than comparing two empty group lists.
    report.edges.retain(|edge| edge.id != 1);
    graph.reset_report();
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    assert!(
        graph
            .placement
            .groups
            .iter()
            .any(|group| group.members == [0, 1, 2]),
        "the sequence must participate in the production layout"
    );
    let measured = geometry(&graph);
    for zoom in [0.20, 0.54, 0.60, 0.66, 1.0, 2.0, 0.40] {
        graph.zoom = zoom;
        graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * zoom;
        frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        assert_eq!(
            geometry(&graph),
            measured,
            "zoom {zoom} changed groups, captions, members or edge paths"
        );
    }
}

#[test]
fn native_state_selection_works_in_both_identity_and_preview_modes() {
    let (ctx, mut graph, report, mut selection) = initialized(GraphMode::States);
    let measured = geometry(&graph);
    for zoom in [0.40, 1.0] {
        graph.zoom = zoom;
        graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * zoom;
        let (_, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let point = graph
            .screen_rect(canvas, graph.placement.nodes[&2])
            .center();
        selection = Selection {
            state: Some(0),
            pc: Some(0),
        };
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut graph,
                &report,
                &mut selection,
                vec![
                    Event::PointerMoved(point),
                    Event::PointerButton {
                        pos: point,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(
            selection,
            Selection {
                state: Some(2),
                pc: None
            }
        );
        assert_eq!(geometry(&graph), measured);
    }
}

#[test]
fn compact_source_groups_show_members_without_presenting_one_state_as_the_group() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut report = crate::tests::report();
    let mut extra = report.cfg[2].clone();
    extra.id = 999;
    report.cfg.push(extra);
    let mut extra_ssa = report.ssa.blocks[2].clone();
    extra_ssa.state = 999;
    extra_ssa.instructions[0].results = vec![9101];
    report.ssa.blocks.push(extra_ssa);
    let mut graph = Graph::default();
    let mut selection = Selection::default();
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    graph.zoom = 0.40;
    graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * graph.zoom;
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let labels = card_text(&output, &graph, canvas);
    assert!(labels.contains(&"P₀:B₂ · 2 states"));
    assert!(labels.contains(&"σ₂ · σ₉₉₉"));
    assert!(!labels.contains(&"σ₂  ·  B₂"));
    assert!(labels.iter().all(|line| !line.contains('%')));
    let measured = geometry(&graph);
    graph.zoom = 1.0;
    let (output, _) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    assert!(text(&output).contains(&"Select an instance for SSA"));
    assert!(!text(&output).iter().any(|line| line.contains("%9101")));
    assert_eq!(geometry(&graph), measured);
}

#[test]
fn zooming_restores_the_selected_disassembly_preview() {
    let (ctx, mut graph, report, mut selection) = initialized(GraphMode::States);
    graph.set_content(NodeView::Disassembly, None);
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let measured = geometry(&graph);
    for (zoom, visible) in [(0.40, false), (1.0, true)] {
        graph.zoom = zoom;
        graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * zoom;
        let (output, _) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        assert_eq!(
            text(&output).iter().any(|line| line.contains("000a  ADD")),
            visible
        );
        let expected = if visible {
            "CFG detail: disassembly preview"
        } else {
            "CFG detail: identities"
        };
        assert!(graph.accessible_summary().contains(expected));
        assert!(
            !graph
                .accessible_summary()
                .contains("CFG detail: SSA preview")
        );
        assert_eq!(geometry(&graph), measured);
    }
}
