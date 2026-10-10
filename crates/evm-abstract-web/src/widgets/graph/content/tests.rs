use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{AnalyzeReply, BlockCoverage, InstructionProgress, SsaTransition};

use super::super::{Graph, projection::GraphMode};
use super::{NodeView, node_text};
use crate::{
    Workspace,
    app::{Selection, View},
};

fn report() -> evm_abstract_protocol::AnalysisReport {
    let mut report = crate::tests::report();
    // Native state IDs are unrelated to vector indices and basic-block IDs.
    report.cfg[2].id = 205;
    report.ssa.blocks[2].state = 205;
    report.edges.iter_mut().for_each(|edge| {
        if edge.to == 2 {
            edge.to = 205;
        }
    });
    report
}

fn frame(
    ctx: &Context,
    graph: &mut Graph,
    report: &evm_abstract_protocol::AnalysisReport,
    selection: &mut Selection,
    events: Vec<Event>,
) -> FullOutput {
    frame_at(
        ctx,
        graph,
        report,
        selection,
        events,
        Vec2::new(1400.0, 900.0),
    )
}

fn frame_at(
    ctx: &Context,
    graph: &mut Graph,
    report: &evm_abstract_protocol::AnalysisReport,
    selection: &mut Selection,
    events: Vec<Event>,
    size: Vec2,
) -> FullOutput {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..RawInput::default()
    };
    graph.prepare_input(&mut input);
    let mut output = ctx.run_ui(input, |ui| graph.show(ui, report, selection));
    output.textures_delta.clear();
    output
}

fn label_position(output: &FullOutput, label: &str) -> Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + Vec2::splat(4.0))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing label {label}"))
}

fn click(
    ctx: &Context,
    graph: &mut Graph,
    report: &evm_abstract_protocol::AnalysisReport,
    selection: &mut Selection,
    label: &str,
) {
    let output = frame(ctx, graph, report, selection, Vec::new());
    let pos = label_position(&output, label);
    for pressed in [true, false] {
        frame(
            ctx,
            graph,
            report,
            selection,
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
}

#[test]
fn default_ssa_and_toolbar_switches_keep_native_state_content_and_selection() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    graph.set_mode(GraphMode::States);
    let mut selection = Selection {
        state: Some(205),
        pc: Some(10),
    };
    frame(&ctx, &mut graph, &report, &mut selection, Vec::new());
    assert_eq!(graph.content, NodeView::Ssa);
    assert!(
        graph.nodes[&205]
            .lines
            .iter()
            .any(|(line, _)| line.contains("%5 = φ(S0:%1 (e1), S1:%2 (e2))")),
        "a fresh CFG must render SSA definitions without a toolbar click"
    );
    click(&ctx, &mut graph, &report, &mut selection, "Disasm");
    assert_eq!(graph.content, NodeView::Disassembly);
    let source_size = graph.nodes[&205].size;
    assert!(
        !graph.nodes[&205]
            .lines
            .iter()
            .any(|(line, _)| line.contains('%'))
    );
    click(&ctx, &mut graph, &report, &mut selection, "SSA");
    assert_eq!(graph.content, NodeView::Ssa);
    let content = &graph.nodes[&205];
    assert!(
        content
            .lines
            .iter()
            .any(|(line, _)| line.contains("%5 = φ(S0:%1 (e1), S1:%2 (e2))"))
    );
    assert!(
        content
            .lines
            .iter()
            .any(|(line, _)| line.contains("%5 = ADD %1, %2 · μ2→μ5"))
    );
    assert!(leaf_tooltip(&report, 205, NodeView::Ssa).contains("exit μ5 · f0 [%5]"));
    assert!(
        content.tooltip.is_empty(),
        "graph construction must not eagerly format tooltips"
    );
    assert_ne!(
        content.size, source_size,
        "mode switch must invalidate measured node content"
    );
    assert_eq!(graph.placement.nodes[&205].size(), content.size);
    assert_eq!(
        selection,
        Selection {
            state: Some(205),
            pc: Some(10)
        }
    );
    click(&ctx, &mut graph, &report, &mut selection, "Disasm");
    assert_eq!(graph.content, NodeView::Disassembly);
    assert_eq!(graph.nodes[&205].size, source_size);
    assert!(!leaf_tooltip(&report, 205, NodeView::Disassembly).contains("%5 ="));
}

#[test]
fn diamond_merge_preview_shows_phi_add_definition_and_exit_use_together() {
    let mut report = report();
    let block = &mut report.ssa.blocks[2];
    block.phis[0].result = 6;
    let template = block.instructions[0].clone();
    block.instructions = [
        (13, "JUMPDEST", vec![], vec![], None),
        (14, "PUSH1", vec![], vec![7], Some("0x3".into())),
        (16, "ADD", vec![7, 6], vec![8], None),
        (17, "STOP", vec![], vec![], None),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (pc, name, operands, results, immediate))| {
        let mut instruction = template.clone();
        instruction.pc = pc;
        instruction.name = name.into();
        instruction.operands = operands;
        instruction.results = results;
        instruction.immediate = immediate;
        instruction.effect_input = 10 + index;
        instruction.effect_result = Some(11 + index);
        instruction.progress = if name == "STOP" {
            InstructionProgress::Dispatched
        } else {
            InstructionProgress::Completed
        };
        instruction
    })
    .collect();
    block.exit_frames = vec![vec![8]];
    block.exit_effect = 14;
    let ctx = Context::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let content = node_text(ui.painter(), &report, &report.cfg[2], NodeView::Ssa);
        for required in ["%6 = φ(", "%8 = ADD %7, %6", "STOP", "exit μ14 · f0 [%8]"] {
            assert!(
                content
                    .lines
                    .iter()
                    .any(|(line, _)| line.contains(required)),
                "the ordinary merge must show {required} without opening hover details"
            );
        }
        assert_eq!(content.lines.len(), 7);
        assert!(!content.lines.iter().any(|(line, _)| line.contains(" rows")));
    });
    output.textures_delta.clear();
}

#[test]
fn ssa_preview_keeps_terminal_phase_and_full_phi_transition_details() {
    let mut report = report();
    let block = &mut report.ssa.blocks[2];
    let phi = block.phis[0].clone();
    block.phis = (0..10)
        .map(|index| {
            let mut phi = phi.clone();
            phi.result = 100 + index;
            phi
        })
        .collect();
    let template = block.instructions[0].clone();
    block.instructions = (0..8)
        .map(|pc| {
            let mut instruction = template.clone();
            instruction.pc = pc;
            instruction
        })
        .collect();
    let call = block.instructions.last_mut().unwrap();
    call.name = "CALL".into();
    call.progress = InstructionProgress::Dispatched;
    call.results.clear();
    report.edges.push(evm_abstract_protocol::CfgEdge {
        id: 88,
        from: 205,
        to: 1,
        kind: evm_abstract_protocol::EdgeKind::Return,
    });
    report.ssa.transitions.push(SsaTransition {
        edge: 88,
        kind: evm_abstract_protocol::EdgeKind::Return,
        operands: vec![1, 2],
        stacks: vec![vec![777]],
        effect_input: 5,
        effect_result: 6,
        result: Some(777),
    });
    let ctx = Context::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let content = node_text(ui.painter(), &report, &report.cfg[2], NodeView::Ssa);
        assert_eq!(content.lines.len(), 8);
        assert!(
            !content
                .lines
                .iter()
                .any(|(line, _)| line.starts_with("0005"))
        );
        assert!(
            content
                .lines
                .iter()
                .any(|(line, _)| line.contains("CALL") && line.contains("Dispatched"))
        );
        assert!(
            !content.lines.iter().any(|(line, _)| line.contains("%777")),
            "deferred call result belongs to its actual transition"
        );
        for required in [
            "%109 = φ",
            "0005",
            "e88 → S1",
            "μ5→μ6 · %777",
            "Arguments [%1, %2] · f0 [%777]",
        ] {
            assert!(
                content.tooltip.contains(required),
                "missing {required}: {}",
                content.tooltip
            );
        }
    });
    output.textures_delta.clear();
}

#[test]
fn ssa_entry_only_and_missing_receipts_never_invent_execution() {
    let ctx = Context::default();
    for coverage in [BlockCoverage::Stale, BlockCoverage::Unexecuted] {
        let mut report = report();
        report.ssa.blocks[2].coverage = coverage;
        report.ssa.blocks[2].incoming_complete = false;
        let mut output = ctx.run_ui(RawInput::default(), |ui| {
            let content = node_text(ui.painter(), &report, &report.cfg[2], NodeView::Ssa);
            assert!(content.title.contains(&format!("{coverage:?}")));
            assert!(content.detail.contains("incoming open"));
            assert!(
                content
                    .lines
                    .iter()
                    .any(|(line, _)| line.starts_with("entry only μ"))
            );
            assert!(!content.tooltip.contains("ADD"));
            assert!(!content.tooltip.contains("exit μ"));
        });
        output.textures_delta.clear();
    }
    let mut report = report();
    report.ssa.blocks.retain(|block| block.state != 205);
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let content = node_text(ui.painter(), &report, &report.cfg[2], NodeView::Ssa);
        assert!(content.lines[0].0.contains("SSA unavailable"));
        assert!(!content.tooltip.contains("ADD"));
    });
    output.textures_delta.clear();
}

#[test]
fn manual_mode_switch_preserves_selected_node_camera_anchor_and_zoom() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    graph.set_mode(GraphMode::States);
    let mut selection = Selection {
        state: Some(205),
        pc: None,
    };
    frame(&ctx, &mut graph, &report, &mut selection, Vec::new());
    graph.fitted = false;
    graph.zoom = 1.2;
    let offset = Vec2::new(12.0, -8.0);
    graph.pan = graph.viewport.unwrap() * 0.5 + offset
        - graph.placement.nodes[&205].center().to_vec2() * graph.zoom;
    click(&ctx, &mut graph, &report, &mut selection, "Disasm");
    assert_eq!(graph.content, NodeView::Disassembly);
    assert!(!graph.fitted);
    assert_eq!(graph.zoom, 1.2);
    let new_offset = graph.pan + graph.placement.nodes[&205].center().to_vec2() * graph.zoom
        - graph.viewport.unwrap() * 0.5;
    assert!((new_offset - offset).length() < 0.01);
}

#[test]
fn a_representation_switch_preserves_initial_fit_scale_and_canvas_anchor() {
    let report = report();
    let ctx = Context::default();
    let mut graph = Graph::default();
    graph.set_mode(GraphMode::States);
    let mut selection = Selection {
        state: Some(205),
        pc: None,
    };
    let small = Vec2::new(340.0, 260.0);
    frame_at(&ctx, &mut graph, &report, &mut selection, Vec::new(), small);
    let zoom = graph.zoom;
    assert!(
        zoom < 1.0,
        "fixture must first fit its SSA content at a reduced scale"
    );
    let anchor = graph.pan + graph.placement.nodes[&205].center().to_vec2() * zoom;
    let ssa_size = graph.nodes[&205].size;
    graph.set_content(NodeView::Disassembly, selection.state);
    frame_at(&ctx, &mut graph, &report, &mut selection, Vec::new(), small);
    assert_eq!(
        graph.zoom, zoom,
        "switching to smaller cards must not automatically magnify the graph"
    );
    assert_ne!(graph.nodes[&205].size, ssa_size);
    assert!(
        (graph.pan + graph.placement.nodes[&205].center().to_vec2() * zoom - anchor).length()
            < 0.01
    );
    let camera = (graph.zoom, graph.pan);
    let nodes = graph.placement.nodes.clone();
    frame(&ctx, &mut graph, &report, &mut selection, Vec::new());
    assert_eq!((graph.zoom, graph.pan), camera);
    assert_eq!(graph.placement.nodes, nodes);
}

#[test]
fn fit_button_centers_both_representations_without_rearranging_nodes() {
    let report = report();
    for content in [NodeView::Disassembly, NodeView::Ssa] {
        let ctx = Context::default();
        let mut graph = Graph::default();
        graph.set_mode(GraphMode::States);
        let expected = Selection {
            state: Some(205),
            pc: Some(10),
        };
        let mut selection = expected;
        frame(&ctx, &mut graph, &report, &mut selection, Vec::new());
        if content == NodeView::Disassembly {
            click(&ctx, &mut graph, &report, &mut selection, "Disasm");
        }
        graph.zoom_at(graph.viewport.unwrap() * 0.5, 1.8);
        graph.pan += Vec2::new(90.0, -70.0);
        assert!(!graph.fitted);
        let nodes = graph.placement.nodes.clone();
        let flow = graph.placement.flow;
        click(&ctx, &mut graph, &report, &mut selection, "Fit graph");
        assert!(graph.fitted);
        assert_eq!(graph.content, content);
        assert_eq!(selection, expected);
        assert_eq!(graph.placement.nodes, nodes);
        assert_eq!(graph.placement.flow, flow);
        let viewport = Rect::from_min_size(Pos2::ZERO, graph.viewport.unwrap());
        let scene = graph.screen_rect(viewport, graph.placement.bounds);
        assert!(viewport.contains_rect(scene));
        assert!(
            (scene.center() - viewport.center()).length() < 0.01,
            "Fit graph left {content:?} off-center: {scene:?} in {viewport:?}"
        );
    }
}

#[test]
fn default_ssa_and_explicit_disassembly_preference_survive_reanalysis() {
    let report = report();
    let ctx = Context::default();
    let mut workspace = Workspace::default();
    workspace.graph.set_mode(GraphMode::States);
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(report.clone()),
    }));
    assert!(workspace.accessible_status().contains("CFG nodes: SSA;"));
    workspace.initial_command();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(report.clone()),
    }));
    assert!(workspace.accessible_status().contains("CFG nodes: SSA;"));
    workspace.view = View::Graph;
    let run = |workspace: &mut Workspace, size: Vec2, events| {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..RawInput::default()
            },
            |ui| {
                workspace.show(ui);
            },
        );
        output.textures_delta.clear();
        output
    };
    let size = Vec2::new(1400.0, 900.0);
    run(&mut workspace, size, Vec::new());
    let output = run(&mut workspace, size, Vec::new());
    assert!(
        output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text().contains("%5 = ADD %1, %2"))),
        "default SSA must be present in the actual CFG cards after reanalysis"
    );
    // A deliberate non-default choice must remain a workspace preference.
    let pos = output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Disasm" => {
                Some(text.pos + Vec2::splat(4.0))
            }
            _ => None,
        })
        .unwrap();
    for pressed in [true, false] {
        run(
            &mut workspace,
            size,
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
    assert!(workspace.accessible_status().contains("CFG nodes: Disasm;"));
    workspace.initial_command();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    for (view, size) in [
        (View::Split, Vec2::new(844.0, 390.0)),
        (View::Ssa, Vec2::new(390.0, 844.0)),
        (View::Graph, size),
    ] {
        workspace.view = view;
        run(&mut workspace, size, Vec::new());
        assert!(workspace.accessible_status().contains("CFG nodes: Disasm;"));
    }
}

fn leaf_tooltip(
    report: &evm_abstract_protocol::AnalysisReport,
    state: usize,
    view: NodeView,
) -> String {
    let index = super::ReportIndex::new(report);
    let block = index.cfg(report, state).unwrap();
    let node = super::DisplayNode {
        id: state,
        members: vec![state],
        program: block.program,
        basic_block: block.basic_block,
        start_pc: block.start_pc,
    };
    super::node_tooltip(report, &index, &node, view)
}
