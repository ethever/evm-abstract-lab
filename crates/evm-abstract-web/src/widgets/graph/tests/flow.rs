use std::{collections::BTreeSet, time::Duration};

use egui::{Color32, Context, FullOutput, Pos2, RawInput, Rect, Shape, Vec2, ViewportId};
use evm_abstract_protocol::{AnalysisReport, CfgEdge, EdgeKind};

use super::{Graph, disassembly_graph, report, snapshot};
use crate::{app::Selection, palette};

fn frame(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
    time: f64,
) -> FullOutput {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 1000.0))),
            time: Some(time),
            ..RawInput::default()
        },
        |ui| graph.show(ui, report, selection),
    );
    output.textures_delta.clear();
    output
}

fn delay(output: &FullOutput) -> Duration {
    output.viewport_output[&ViewportId::ROOT].repaint_delay
}

fn arrows(output: &FullOutput) -> Vec<Vec<Pos2>> {
    let colors =
        [palette::ACCENT, palette::BLUE].map(|color| color.lerp_to_gamma(Color32::WHITE, 0.7));
    output
        .shapes
        .iter()
        .filter_map(|item| match &item.shape {
            Shape::Path(path) if path.closed && colors.contains(&path.fill) => {
                Some(path.points.clone())
            }
            _ => None,
        })
        .collect()
}

fn cycle_report() -> AnalysisReport {
    let mut report = report();
    report.cfg.truncate(4);
    report.edges = vec![
        CfgEdge {
            id: 0,
            from: 0,
            to: 7,
            kind: EdgeKind::Jump,
        },
        CfgEdge {
            id: 1,
            from: 7,
            to: 14,
            kind: EdgeKind::Fallthrough,
        },
        CfgEdge {
            id: 2,
            from: 14,
            to: 7,
            kind: EdgeKind::Jump,
        },
        CfgEdge {
            id: 3,
            from: 7,
            to: 7,
            kind: EdgeKind::BranchTrue,
        },
        CfgEdge {
            id: 4,
            from: 14,
            to: 21,
            kind: EdgeKind::Fallthrough,
        },
    ];
    report
}

#[test]
fn selection_animates_native_incoming_outgoing_back_and_self_edges_without_relayout() {
    let report = cycle_report();
    let ctx = Context::default();
    palette::configure(&ctx);
    let mut graph = disassembly_graph();
    let mut selection = Selection::default();
    for index in 0..4 {
        frame(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            f64::from(index) * 0.1,
        );
    }
    let scene = snapshot(&graph);
    selection.state = Some(7);
    let before = frame(&ctx, &mut graph, &report, &mut selection, 1.1);
    let after = frame(&ctx, &mut graph, &report, &mut selection, 1.2);
    assert_eq!(graph.highlighted_edges, BTreeSet::from([0, 1, 2, 3]));
    assert!(!arrows(&before).is_empty());
    assert_ne!(
        arrows(&before),
        arrows(&after),
        "time must move the flowing heads"
    );
    assert!(delay(&after) <= super::super::edges::FLOW_FRAME_INTERVAL);
    assert_eq!(
        snapshot(&graph),
        scene,
        "animation cannot rebuild or move the scene"
    );
    assert!(!graph.layout_pending && !graph.display_dirty);

    selection.state = Some(21);
    let switched = frame(&ctx, &mut graph, &report, &mut selection, 1.3);
    assert_eq!(graph.highlighted_edges, BTreeSet::from([4]));
    assert!(!arrows(&switched).is_empty());
    assert_eq!(snapshot(&graph), scene);
    // The previous node's green jumping/back/self routes now have their usual
    // static strokes and endpoint arrows, with no bright moving overlay.
    let old_flow_color = palette::ACCENT.lerp_to_gamma(Color32::WHITE, 0.7);
    assert!(switched.shapes.iter().all(|item| !matches!(
        &item.shape,
        Shape::Path(path) if path.closed && path.fill == old_flow_color
    )));
}

#[test]
fn cleared_selection_and_offscreen_highlights_leave_the_graph_idle() {
    let report = cycle_report();
    let ctx = Context::default();
    palette::configure(&ctx);
    let mut graph = disassembly_graph();
    let mut selection = Selection::default();
    let mut output = FullOutput::default();
    for index in 0..5 {
        output = frame(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            f64::from(index) * 0.1,
        );
    }
    assert!(arrows(&output).is_empty());
    assert_eq!(
        delay(&output),
        Duration::MAX,
        "unselected settled graph must stay idle"
    );

    selection.state = Some(7);
    output = frame(&ctx, &mut graph, &report, &mut selection, 1.0);
    assert!(!arrows(&output).is_empty());
    assert!(delay(&output) <= super::super::edges::FLOW_FRAME_INTERVAL);
    let scene = snapshot(&graph);
    graph.pan += Vec2::splat(100_000.0);
    graph.fitted = false;
    for index in 0..3 {
        output = frame(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            1.1 + f64::from(index) * 0.1,
        );
    }
    assert!(
        !graph.highlighted_edges.is_empty(),
        "selection remains native even offscreen"
    );
    assert!(arrows(&output).is_empty());
    assert_eq!(
        delay(&output),
        Duration::MAX,
        "hidden highlights must not request animation frames"
    );
    assert_eq!(snapshot(&graph), scene);

    graph.pan -= Vec2::splat(100_000.0);
    selection.state = None;
    for index in 0..3 {
        output = frame(
            &ctx,
            &mut graph,
            &report,
            &mut selection,
            2.0 + f64::from(index) * 0.1,
        );
    }
    assert!(graph.highlighted_edges.is_empty());
    assert!(arrows(&output).is_empty());
    assert_eq!(
        delay(&output),
        Duration::MAX,
        "clearing the selection must stop animation"
    );
    assert_eq!(snapshot(&graph), scene);
}
