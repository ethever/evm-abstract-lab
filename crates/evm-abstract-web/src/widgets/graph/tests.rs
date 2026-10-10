use egui::{Context, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, CfgBlock, CfgEdge, DisasmInstruction, EdgeKind,
    Fork, SsaReport,
};

use super::{Graph, layout::Flow, node_text};
use crate::app::Selection;

#[derive(Debug, PartialEq)]
struct SceneSnapshot {
    nodes: std::collections::BTreeMap<usize, Rect>,
    edges: Vec<(usize, [Vec<Pos2>; 2], Rect)>,
    bounds: Rect,
    flow: Flow,
}

fn snapshot(graph: &Graph) -> SceneSnapshot {
    SceneSnapshot {
        nodes: graph.placement.nodes.clone(),
        edges: graph
            .placement
            .edges
            .iter()
            .map(|(id, edge)| {
                (
                    *id,
                    [edge.to_label.clone(), edge.to_target.clone()],
                    edge.label,
                )
            })
            .collect(),
        bounds: graph.placement.bounds,
        flow: graph.placement.flow,
    }
}

#[test]
fn frozen_scene_and_camera_survive_wide_narrow_wide_viewports() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    for manual in [false, true] {
        let mut graph = disassembly_graph();
        render(&ctx, &mut graph, &report, Vec2::new(1100.0, 450.0));
        if manual {
            graph.zoom_at(Vec2::new(33.0, 91.0), 0.7);
            graph.pan += Vec2::new(19.0, -23.0);
        }
        let scene = snapshot(&graph);
        let camera = (graph.zoom, graph.pan);
        for size in [
            Vec2::new(390.0, 720.0),
            Vec2::new(844.0, 200.0),
            Vec2::new(1100.0, 450.0),
        ] {
            render(&ctx, &mut graph, &report, size);
            assert_eq!(
                snapshot(&graph),
                scene,
                "resize changed the initialized scene: {size:?}, manual={manual}"
            );
            assert_eq!(
                (graph.zoom, graph.pan),
                camera,
                "resize changed canvas-relative camera coordinates: {size:?}, manual={manual}"
            );
        }
    }
}

#[test]
fn frozen_scene_fit_changes_camera_without_repacking_nodes_or_edges() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(1100.0, 450.0));
    let scene = snapshot(&graph);
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 500.0));
    graph.queue_fit();
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 500.0));
    assert_eq!(
        snapshot(&graph),
        scene,
        "Fit must not choose another orientation or rank wrapping"
    );
    assert_scene_fits(&graph);
}

#[test]
fn frozen_scene_waits_for_a_usable_initial_canvas() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::splat(1.0));
    assert!(
        graph.placement.nodes.is_empty(),
        "a temporary 1px canvas must not determine the permanent layout"
    );
    assert!(graph.layout_pending);
    render(&ctx, &mut graph, &report, Vec2::new(900.0, 600.0));
    assert_eq!(graph.placement.nodes.len(), report.cfg.len());
    assert_scene_fits(&graph);
}

#[test]
fn a_sizing_pass_cannot_consume_the_first_layout_or_fit() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        ui.scope_builder(egui::UiBuilder::new().sizing_pass(), |ui| {
            graph.show(ui, &report, &mut Selection::default());
        });
    });
    output.textures_delta.clear();
    assert!(graph.placement.nodes.is_empty());
    assert!(graph.layout_pending && graph.fit_pending);
    render(&ctx, &mut graph, &report, Vec2::new(900.0, 600.0));
    assert_eq!(graph.placement.nodes.len(), report.cfg.len());
    assert_scene_fits(&graph);
}

#[test]
fn a_new_report_gets_a_new_initial_layout_and_fit() {
    let mut report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(1100.0, 450.0));
    let old_scene = snapshot(&graph);
    graph.zoom_at(Vec2::new(33.0, 91.0), 0.7);
    graph.pan += Vec2::new(90.0, -25.0);
    graph.reset_report();
    assert!(graph.placement.nodes.is_empty());
    assert!(graph.viewport.is_none());
    assert_eq!(graph.content, super::NodeView::Disassembly);
    let instruction = report.cfg[0].instructions[0].clone();
    report.cfg[0].instructions.push(instruction);
    let size = Vec2::new(390.0, 720.0);
    render(&ctx, &mut graph, &report, size);
    let mut fresh = disassembly_graph();
    render(&ctx, &mut fresh, &report, size);
    assert_eq!(snapshot(&graph), snapshot(&fresh));
    assert_eq!((graph.zoom, graph.pan), (fresh.zoom, fresh.pan));
    assert_ne!(snapshot(&graph), old_scene);
    assert_scene_fits(&graph);
}

fn report() -> AnalysisReport {
    let cfg: Vec<_> = (0..9)
        .map(|index| CfgBlock {
            id: index * 7,
            basic_block: index,
            start_pc: Some(index * 2),
            context: vec![],
            frame_depth: 1,
            code_address: "0x11".into(),
            storage_address: "0x11".into(),
            program: Some(0),
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
    let disassembly = cfg
        .iter()
        .map(|block| evm_abstract_protocol::DisasmBlock {
            id: block.basic_block,
            start_pc: block.start_pc.unwrap(),
            instructions: block.instructions.clone(),
        })
        .collect::<Vec<_>>();
    let mut report = AnalysisReport {
        schema_version: evm_abstract_protocol::SCHEMA_VERSION,
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 18,
        status: AnalysisStatus::Converged,
        transfers: 9,
        disassembly: disassembly.clone(),
        programs: vec![crate::tests::source_program(
            0,
            cfg[0].code_address.clone(),
            disassembly,
        )],
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
        ..crate::tests::empty_report()
    };
    report.metadata.root_program = Some(0);
    report
}

// These fixtures exercise source-card geometry and deliberately carry no SSA.
// Default SSA behavior is tested with real typed SSA blocks in content/tests.rs.
fn disassembly_graph() -> Graph {
    let mut graph = Graph {
        content: super::content::NodeView::Disassembly,
        ..Graph::default()
    };
    graph.set_mode(crate::widgets::GraphMode::States);
    graph
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
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
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
        let label = route.label;
        assert!(
            graph
                .placement
                .nodes
                .values()
                .all(|node| !node.intersects(label)),
            "edge label overlaps a node"
        );
    }
    let mut graph = disassembly_graph();
    render_with_chrome(&ctx, &mut graph, &report, Vec2::new(390.0, 844.0));
    assert_eq!(graph.placement.flow, Flow::Down);
    assert_scene_fits(&graph);
    let mut graph = disassembly_graph();
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
fn initial_layout_adapts_to_the_first_canvas_size() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(1100.0, 450.0));
    assert_scene_fits(&graph);
    let wide = graph.placement.nodes.clone();
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 720.0));
    assert_scene_fits(&graph);
    assert_ne!(
        wide, graph.placement.nodes,
        "a fresh portrait scene may pack the broad rank differently"
    );
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(844.0, 200.0));
    assert_scene_fits(&graph);
    assert!(graph.fitted);
    assert_eq!(graph.placement.nodes.len(), 9);
}

#[test]
fn fit_centers_the_complete_scene_in_a_roomy_canvas() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    for size in [Vec2::new(1440.0, 1200.0), Vec2::new(1440.0, 1600.0)] {
        render(&ctx, &mut graph, &report, size);
        graph.queue_fit();
        render(&ctx, &mut graph, &report, size);
        assert_eq!(graph.zoom, 1.0);
        assert_scene_fits(&graph);
    }
}

#[test]
fn resize_preserves_manual_canvas_coordinates_and_fit_centers_the_scene() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(900.0, 500.0));
    let previous = graph.viewport.unwrap();
    graph.zoom_at(previous * 0.5, 1.4);
    graph.pan += Vec2::new(17.0, -21.0);
    let pan = graph.pan;
    let zoom = graph.zoom;
    let nodes = graph.placement.nodes.clone();
    graph.update_viewport(&report, Vec2::new(390.0, 600.0));
    assert_eq!(graph.pan, pan);
    assert_eq!(graph.zoom, zoom);
    assert_eq!(graph.placement.nodes, nodes);
    assert!(!graph.fitted);
    graph.queue_fit();
    graph.update_viewport(&report, Vec2::new(390.0, 600.0));
    assert!(graph.fitted);
    assert_scene_fits(&graph);
    graph.zoom_at(Vec2::ZERO, 1.0);
    assert!(
        graph.fitted,
        "hover without a zoom gesture must retain Fit mode"
    );
}

#[test]
fn focus_selected_centers_its_measured_rect_and_enters_manual_mode() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
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
    assert!(!graph.fitted);
    assert_eq!(selection.state, Some(49));
}

#[test]
fn content_measurement_shrinks_short_nodes_and_retains_empty_frame_semantics() {
    let report = report();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
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
        source.program = None;
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
    crate::notation::initialize_fonts(&ctx);
    let mut graph = disassembly_graph();
    render(&ctx, &mut graph, &report, Vec2::new(390.0, 500.0));
    for id in [42, 44] {
        let route = &graph.placement.edges[&id];
        assert!(
            route
                .to_label
                .iter()
                .chain(&route.to_target)
                .all(|point| point.is_finite() && graph.placement.bounds.contains(*point))
        );
        assert_ne!(
            route.to_target[route.to_target.len() - 1],
            route.to_target[route.to_target.len() - 2]
        );
        assert!(graph.placement.bounds.contains_rect(route.label));
    }
    assert_scene_fits(&graph);
}

#[test]
fn painted_virtual_labels_remain_outside_state_cards_when_zoomed() {
    let mut report = diamond();
    report.edges.push(CfgEdge {
        id: 987_654,
        from: 21,
        to: 0,
        kind: EdgeKind::Return,
    });
    for density in [1.0, 2.0] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut graph = disassembly_graph();
        render(&ctx, &mut graph, &report, Vec2::new(1400.0, 1000.0));
        let scene = snapshot(&graph);
        for zoom in [0.36, 0.5, 1.0, 2.5] {
            graph.zoom = zoom;
            graph.pan = Vec2::splat(20.0) - graph.placement.bounds.min.to_vec2() * zoom;
            graph.fitted = false;
            let size = (graph.placement.bounds.size() * zoom + Vec2::new(40.0, 160.0))
                .max(Vec2::new(900.0, 900.0));
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..RawInput::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(density);
            let mut canvas = Rect::NOTHING;
            let mut output = ctx.run_ui(input, |ui| {
                let available = ui.available_rect_before_wrap();
                graph.show(ui, &report, &mut Selection::default());
                canvas = Rect::from_min_size(
                    available.right_bottom() - graph.viewport.unwrap(),
                    graph.viewport.unwrap(),
                );
            });
            output.textures_delta.clear();
            assert_eq!(
                snapshot(&graph),
                scene,
                "rendering must not relocate label vertices"
            );
            for edge in &report.edges {
                let name = super::edge_label(edge.id, edge.kind);
                let text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == name => {
                            Some(Rect::from_min_size(text.pos, text.galley.size()))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        panic!("missing readable label {name}, DPR{density}, zoom{zoom}")
                    });
                let label = graph.screen_rect(canvas, graph.placement.edges[&edge.id].label);
                assert!(
                    label.contains_rect(text),
                    "{name} text{text:?} exceeds label{label:?}"
                );
                for node in graph.placement.nodes.values() {
                    let node = graph.screen_rect(canvas, *node);
                    assert!(
                        !node.intersect(label).is_positive(),
                        "{name} label{label:?} is covered by state card{node:?}"
                    );
                }
            }
        }
    }
}
