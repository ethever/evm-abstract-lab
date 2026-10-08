use egui::{Context, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, CfgBlock, CfgEdge, DisasmInstruction, EdgeKind,
    Fork, SsaReport,
};

use super::{Graph, layout::Flow, node_text};
use crate::app::Selection;

fn report() -> AnalysisReport {
    let cfg = (0..9)
        .map(|index| CfgBlock {
            id: index * 7,
            basic_block: index,
            start_pc: Some(index * 2),
            context: vec![],
            frame_depth: 1,
            code_address: "0x11".into(),
            instructions: vec![DisasmInstruction {
                pc: index * 2,
                opcode: 0,
                name: "STOP".into(),
                immediate: None,
                size: 1,
                valid: true,
                stack_inputs: 0,
                stack_outputs: 0,
            }],
            entry_stack: vec![],
            exit_stack: vec![],
            executed_pcs: vec![index * 2],
        })
        .collect();
    AnalysisReport {
        schema_version: 1,
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 18,
        status: AnalysisStatus::Converged,
        transfers: 9,
        disassembly: vec![],
        cfg,
        edges: (1..9)
            .map(|index| CfgEdge {
                id: index * 3,
                from: 0,
                to: index * 7,
                kind: EdgeKind::Jump,
            })
            .collect(),
        ssa: SsaReport {
            complete: true,
            value_count: 0,
            effect_count: 0,
            blocks: vec![],
            transitions: vec![],
            deferred_edges: vec![],
        },
        diagnostics: vec![],
        frontiers: vec![],
    }
}

fn render(ctx: &Context, graph: &mut Graph, report: &AnalysisReport, size: Vec2) {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..RawInput::default()
        },
        |ui| {
            graph.show(ui, report, &mut Selection::default());
        },
    );
    output.textures_delta.clear();
}

fn assert_scene_fits(graph: &Graph) {
    let viewport = Rect::from_min_size(Pos2::ZERO, graph.viewport.unwrap());
    let scene = graph.screen_rect(viewport, graph.placement.bounds);
    assert!(
        viewport.contains_rect(scene),
        "scene {scene:?} escapes viewport {viewport:?}"
    );
    assert!(
        (scene.center() - viewport.center()).length() < 0.01,
        "complete scene {scene:?} is not centered in viewport {viewport:?}"
    );
}

fn diamond() -> AnalysisReport {
    let mut report = report();
    report.cfg.truncate(4);
    for (block, names) in report.cfg.iter_mut().zip([
        vec!["PUSH0", "CALLDATALOAD", "PUSH1", "JUMPI"],
        vec!["JUMPDEST", "PUSH1"],
        vec!["PUSH1", "PUSH1", "JUMP"],
        vec!["JUMPDEST", "PUSH1", "ADD", "STOP"],
    ]) {
        block.instructions = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| DisasmInstruction {
                pc: index,
                name: name.into(),
                ..block.instructions[0].clone()
            })
            .collect();
    }
    report.edges = vec![
        CfgEdge {
            id: 0,
            from: 0,
            to: 7,
            kind: EdgeKind::BranchTrue,
        },
        CfgEdge {
            id: 1,
            from: 0,
            to: 14,
            kind: EdgeKind::BranchFalse,
        },
        CfgEdge {
            id: 2,
            from: 7,
            to: 21,
            kind: EdgeKind::Fallthrough,
        },
        CfgEdge {
            id: 3,
            from: 14,
            to: 21,
            kind: EdgeKind::Jump,
        },
    ];
    report
}

fn render_with_chrome(ctx: &Context, graph: &mut Graph, report: &AnalysisReport, screen: Vec2) {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            ..RawInput::default()
        },
        |ui| {
            // Match the real workbench's header/input and bottom completion strip;
            // the graph's own heading and toolbar further reduce canvas height.
            egui::Panel::top("test_header")
                .exact_size(72.0)
                .show(ui, |_| {});
            egui::Panel::bottom("test_status")
                .exact_size(24.0)
                .show(ui, |_| {});
            graph.show(ui, report, &mut Selection::default());
        },
    );
    output.textures_delta.clear();
}

#[test]
fn short_wide_diamond_uses_horizontal_ranks_at_native_text_scale() {
    let report = diamond();
    let ctx = Context::default();
    let mut graph = Graph::default();
    render_with_chrome(&ctx, &mut graph, &report, Vec2::new(844.0, 390.0));
    assert!(
        graph.viewport.unwrap().y < 260.0,
        "must exercise the real short canvas"
    );
    assert_eq!(graph.placement.flow, Flow::Right);
    assert_eq!(
        graph.zoom, 1.0,
        "horizontal room should preserve native text size"
    );
    assert_scene_fits(&graph);
    for route in graph.placement.edges.values() {
        let label = Rect::from_min_size(route.label, Vec2::new(50.0, 14.0));
        assert!(
            graph
                .placement
                .nodes
                .values()
                .all(|node| !node.intersects(label)),
            "edge label overlaps a node"
        );
    }
    render_with_chrome(&ctx, &mut graph, &report, Vec2::new(390.0, 844.0));
    assert_eq!(graph.placement.flow, Flow::Down);
    assert_scene_fits(&graph);
    render_with_chrome(&ctx, &mut graph, &report, Vec2::new(1440.0, 900.0));
    assert_eq!(
        graph.placement.flow,
        Flow::Down,
        "vertical must win equal native-size fits"
    );
    assert_eq!(graph.zoom, 1.0);
    assert_scene_fits(&graph);
}

#[test]
fn real_canvas_resize_reflows_and_fits_nodes_edges_and_labels() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    render(&ctx, &mut graph, &report, Vec2::new(1100.0, 450.0));
    assert_scene_fits(&graph);
    let wide = graph.placement.nodes.clone();
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 720.0));
    assert_scene_fits(&graph);
    assert_ne!(
        wide, graph.placement.nodes,
        "portrait must repack the broad rank"
    );
    render(&ctx, &mut graph, &report, Vec2::new(844.0, 200.0));
    assert_scene_fits(&graph);
    assert!(graph.automatic);
    assert_eq!(graph.placement.nodes.len(), 9);
}

#[test]
fn fit_centers_the_complete_scene_in_a_roomy_canvas() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    for size in [Vec2::new(1440.0, 1200.0), Vec2::new(1440.0, 1600.0)] {
        render(&ctx, &mut graph, &report, size);
        assert_eq!(graph.zoom, 1.0);
        assert_scene_fits(&graph);
    }
}

#[test]
fn resize_preserves_manual_world_center_and_fit_restores_automatic_mode() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    render(&ctx, &mut graph, &report, Vec2::new(900.0, 500.0));
    let previous = graph.viewport.unwrap();
    graph.zoom_at(previous * 0.5, 1.4);
    graph.pan += Vec2::new(17.0, -21.0);
    let center = (previous * 0.5 - graph.pan) / graph.zoom;
    let zoom = graph.zoom;
    let nodes = graph.placement.nodes.clone();
    graph.update_viewport(&report, Vec2::new(390.0, 600.0));
    let resized_center = (graph.viewport.unwrap() * 0.5 - graph.pan) / graph.zoom;
    assert!((center - resized_center).length() < 0.001);
    assert_eq!(graph.zoom, zoom);
    assert_eq!(graph.placement.nodes, nodes);
    assert!(!graph.automatic);
    graph.restore_automatic();
    graph.update_viewport(&report, Vec2::new(390.0, 600.0));
    assert!(graph.automatic);
    assert_scene_fits(&graph);
    graph.zoom_at(Vec2::ZERO, 1.0);
    assert!(
        graph.automatic,
        "hover without a zoom gesture must retain automatic mode"
    );
}

#[test]
fn focus_selected_centers_its_measured_rect_and_enters_manual_mode() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    let screen = Vec2::new(700.0, 380.0);
    render(&ctx, &mut graph, &report, screen);
    graph.focus_pending = true;
    let mut selection = Selection {
        state: Some(49),
        pc: None,
    };
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
            ..RawInput::default()
        },
        |ui| graph.show(ui, &report, &mut selection),
    );
    output.textures_delta.clear();
    let world_center = (graph.viewport.unwrap() * 0.5 - graph.pan) / graph.zoom;
    assert!((world_center - graph.placement.nodes[&49].center().to_vec2()).length() < 0.001);
    assert!(!graph.automatic);
    assert_eq!(selection.state, Some(49));
}

#[test]
fn content_measurement_shrinks_short_nodes_and_retains_empty_frame_semantics() {
    let report = report();
    let ctx = Context::default();
    let mut sizes = None;
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let short = node_text(
            ui.painter(),
            &report,
            &report.cfg[0],
            super::NodeView::Disassembly,
        );
        let mut source = report.cfg[0].clone();
        source.instructions = vec![source.instructions[0].clone(); 6];
        source.instructions[0].name = "DELEGATECALL".into();
        source.instructions[0].immediate = Some("0xabcdef0123456789abcdef".into());
        let long = node_text(ui.painter(), &report, &source, super::NodeView::Disassembly);
        source.instructions.clear();
        source.start_pc = None;
        source.frame_depth = 2;
        let empty = node_text(ui.painter(), &report, &source, super::NodeView::Disassembly);
        assert!(empty.lines.is_empty());
        assert!(empty.detail.starts_with("no bytecode"));
        assert!(empty.size.y < short.size.y);
        sizes = Some((short.size, long.size));
    });
    output.textures_delta.clear();
    let (short, long) = sizes.unwrap();
    assert!(
        short.y < 60.0,
        "one instruction must not reserve a 148px card"
    );
    assert!(long.y > short.y && long.x > short.x);
    assert!(long.y < 120.0);
}

#[test]
fn cycles_self_edges_and_label_detours_are_inside_fit_bounds() {
    let mut report = report();
    report.edges.extend([
        CfgEdge {
            id: 42,
            from: 7,
            to: 0,
            kind: EdgeKind::Jump,
        },
        CfgEdge {
            id: 44,
            from: 7,
            to: 7,
            kind: EdgeKind::BranchTrue,
        },
    ]);
    let ctx = Context::default();
    let mut graph = Graph::default();
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 500.0));
    for id in [42, 44] {
        let route = &graph.placement.edges[&id];
        assert!(
            route
                .points
                .iter()
                .all(|point| point.is_finite() && graph.placement.bounds.contains(*point))
        );
        assert_ne!(
            route.points[route.points.len() - 1],
            route.points[route.points.len() - 2]
        );
        assert!(
            graph
                .placement
                .bounds
                .contains(route.label + Vec2::new(35.0, 12.0))
        );
    }
    assert_scene_fits(&graph);
}
