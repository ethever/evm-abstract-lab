use crate::tests::AnalysisReport;
use std::collections::BTreeMap;

use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    BlockCoverage, CfgEdge, EdgeKind, Frontier, FrontierDetails, FrontierKind, InstructionProgress,
    Outcome, OutcomeKind, SsaInstruction,
};

use super::super::{Graph, content::NodeView, layout::Group, projection::GraphMode};
use crate::{app::Selection, ssa_display::SsaDisplay};

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
        graph.show(ui, &(report).view(), selection);
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
                .scene
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
        nodes: graph.scene.placement.nodes.clone(),
        edges: graph
            .scene
            .placement
            .edges
            .iter()
            .map(|(id, route)| (*id, route.path.clone(), route.label))
            .collect(),
        groups: graph.scene.placement.groups.clone(),
        bounds: graph.scene.placement.bounds,
        sizes: graph
            .scene
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
    graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2();
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
    graph.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
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
        graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * zoom;
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
    graph.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    assert!(
        graph
            .scene
            .placement
            .groups
            .iter()
            .any(|group| group.members == [0, 1, 2]),
        "the sequence must participate in the production layout"
    );
    let measured = geometry(&graph);
    for zoom in [0.20, 0.54, 0.60, 0.66, 1.0, 2.0, 0.40] {
        graph.zoom = zoom;
        graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * zoom;
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
        graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * zoom;
        let (_, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let point = graph
            .screen_rect(canvas, graph.scene.placement.nodes[&2])
            .center();
        selection = Selection {
            value: None,
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
                value: None,
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
    graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * graph.zoom;
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
        graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * zoom;
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

fn receipt(pc: usize, opcode: u8, name: &str) -> SsaInstruction {
    SsaInstruction {
        operation: crate::tests::opcode_operation(opcode, name, &[1, 2], None),
        pc,
        opcode,
        name: name.into(),
        immediate: None,
        operands: vec![1, 2],
        results: vec![],
        fault: false,
        progress: InstructionProgress::Completed,
        effect_input: pc,
        effect_result: Some(pc + 1),
    }
}

fn with_receipts(instructions: Vec<SsaInstruction>) -> AnalysisReport {
    let mut report = crate::tests::report();
    report.ssa.blocks[2].phis.clear();
    report.ssa.blocks[2].instructions = instructions;
    report
}

fn native_graph(report: &AnalysisReport) -> (Context, Graph, Selection) {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    graph.set_mode(GraphMode::States);
    let mut selection = Selection::default();
    frame(&ctx, &mut graph, report, &mut selection, vec![]);
    graph.fitted = false;
    graph.zoom = 1.0;
    graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2();
    (ctx, graph, selection)
}

#[test]
fn ssa_detail_switch_paints_stack_receipts_and_phases_with_frozen_geometry() {
    let mut call = receipt(105, 0xf1, "CALL");
    call.progress = InstructionProgress::Dispatched;
    let report = with_receipts(vec![
        receipt(100, 0x5b, "JUMPDEST"),
        receipt(101, 0x80, "DUP1"),
        receipt(102, 0x9f, "SWAP16"),
        receipt(103, 0x50, "POP"),
        receipt(104, 0x01, "ADD"),
        call,
    ]);
    let original = report.ssa.blocks[2].instructions.clone();
    let (ctx, mut graph, mut selection) = native_graph(&report);
    let measured = geometry(&graph);
    let edges = graph.scene.display.edges.clone();
    let labels = graph.scene.edge_labels.clone();
    let camera = (graph.zoom, graph.pan);
    for display in [SsaDisplay::Compact, SsaDisplay::Full, SsaDisplay::Compact] {
        graph.set_ssa_display(display, selection);
        let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let lines = card_text(&output, &graph, canvas);
        for name in ["JUMPDEST", "ADD", "CALL"] {
            assert!(
                lines.iter().any(|line| line.contains(name)),
                "missing {name}"
            );
        }
        for name in ["DUP1", "SWAP16", "POP"] {
            assert_eq!(
                lines.iter().any(|line| line.contains(name)),
                display == SsaDisplay::Full,
                "unexpected {name} in {display:?}"
            );
        }
        let add = lines.iter().find(|line| line.contains("  ADD")).unwrap();
        assert_eq!(add.contains('μ'), display == SsaDisplay::Full);
        assert_eq!(add.contains("Completed"), display == SsaDisplay::Full);
        assert!(lines.iter().any(|line| line.contains("CALL")
            && line.contains("Dispatched")
            && line.contains('μ')));
        assert_eq!(geometry(&graph), measured);
        assert_eq!(graph.scene.display.edges, edges);
        assert_eq!(graph.scene.edge_labels, labels);
        assert_eq!((graph.zoom, graph.pan), camera);
        assert!(!graph.scene.layout_pending);
        assert!(!graph.fit_pending);
    }
    assert_eq!(report.ssa.blocks[2].instructions, original);
    let node = &graph.scene.display.nodes[graph.scene.node_positions[&2]];
    let tooltip = super::super::content::node_tooltip(
        &(report).view(),
        graph.scene.index.as_ref().unwrap(),
        node,
        NodeView::Ssa,
    );
    for raw in ["DUP1", "SWAP16", "POP", "Completed", "μ₁₀₄→μ₁₀₅"] {
        assert!(tooltip.contains(raw), "tooltip lost {raw}");
    }
}

#[test]
fn compact_ssa_paints_faults_unfinished_stack_operations_and_effects() {
    let mut dup = receipt(100, 0x8f, "DUP16");
    dup.fault = true;
    let mut swap = receipt(101, 0x90, "SWAP1");
    swap.progress = InstructionProgress::Dispatched;
    let mut pop = receipt(102, 0x50, "POP");
    pop.progress = InstructionProgress::Started;
    pop.effect_result = None;
    let report = with_receipts(vec![
        dup,
        swap,
        pop,
        receipt(103, 0x55, "SSTORE"),
        receipt(104, 0x5b, "JUMPDEST"),
        receipt(105, 0xff, "SELFDESTRUCT"),
    ]);
    let (ctx, mut graph, mut selection) = native_graph(&report);
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let lines = card_text(&output, &graph, canvas);
    for required in [
        "DUP16",
        "SWAP1",
        "POP",
        "SSTORE",
        "JUMPDEST",
        "SELFDESTRUCT",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(required)),
            "missing {required}"
        );
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("DUP16") && line.contains("fault") && line.contains('μ'))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("SWAP1") && line.contains("Dispatched"))
    );
    assert!(
        lines.iter().any(|line| line.contains("POP")
            && line.contains("pending")
            && line.contains("Started"))
    );
    assert!(
        graph.scene.nodes[&2]
            .lines
            .iter()
            .any(|(line, color)| line.contains("DUP16") && *color == crate::palette::ERROR)
    );
}

#[test]
fn compact_preview_fills_filtered_quota_and_temporarily_reveals_a_buried_native_pc() {
    let mut instructions: Vec<_> = (0..1024).map(|pc| receipt(pc, 0x9f, "SWAP16")).collect();
    instructions.extend((1024..1032).map(|pc| receipt(pc, 0x01, "ADD")));
    instructions.push(receipt(1032, 0x00, "STOP"));
    let report = with_receipts(instructions);
    let (ctx, mut graph, mut selection) = native_graph(&report);
    let measured = geometry(&graph);
    let camera = (graph.zoom, graph.pan);
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let lines = card_text(&output, &graph, canvas);
    assert_eq!(
        lines.iter().filter(|line| line.contains("  ADD")).count(),
        5
    );
    assert!(lines.iter().any(|line| line.contains("STOP")));
    assert!(lines.iter().all(|line| !line.contains("SWAP16")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("1024 details hidden"))
    );
    for selected_pc in [Some(512), Some(700), None] {
        selection = Selection {
            value: None,
            state: Some(2),
            pc: selected_pc,
        };
        graph.set_ssa_display(SsaDisplay::Compact, selection);
        let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let lines = card_text(&output, &graph, canvas);
        let revealed: Vec<_> = lines
            .iter()
            .filter(|line| line.contains("SWAP16"))
            .collect();
        assert_eq!(revealed.len(), usize::from(selected_pc.is_some()));
        if let Some(pc) = selected_pc {
            assert!(revealed[0].starts_with(&format!("{pc:04x}")));
        }
        assert!(lines.iter().any(|line| line.contains("STOP")));
        assert_eq!(graph.scene.nodes[&2].lines.len(), 8);
        assert_eq!(geometry(&graph), measured);
        assert_eq!((graph.zoom, graph.pan), camera);
        assert!(!graph.scene.layout_pending);
    }
}

#[test]
fn selected_stack_row_fits_natural_reserved_width_and_retains_full_tooltip() {
    let mut instructions: Vec<_> = (0..20).map(|pc| receipt(pc, 0x01, "ADD")).collect();
    let mut wide = receipt(20, 0x9f, "SWAP16");
    wide.operands = vec![usize::MAX, usize::MAX - 1];
    crate::tests::refresh_operation(&mut wide);
    instructions.push(wide);
    instructions.push(receipt(21, 0x00, "STOP"));
    let report = with_receipts(instructions);
    let (ctx, mut graph, _) = native_graph(&report);
    let measured = geometry(&graph);
    let camera = (graph.zoom, graph.pan);
    let mut selection = Selection {
        value: None,
        state: Some(2),
        pc: Some(20),
    };
    graph.set_ssa_display(SsaDisplay::Compact, selection);
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let lines = card_text(&output, &graph, canvas);
    let revealed = lines.iter().find(|line| line.contains("SWAP16")).unwrap();
    assert!(revealed.starts_with("0014  SWAP16"));
    assert!(
        revealed.ends_with('…'),
        "wide native arguments must stay within the reserved card"
    );
    assert_eq!(geometry(&graph), measured);
    assert_eq!((graph.zoom, graph.pan), camera);
    let node = &graph.scene.display.nodes[graph.scene.node_positions[&2]];
    let tooltip = super::super::content::node_tooltip(
        &(report).view(),
        graph.scene.index.as_ref().unwrap(),
        node,
        NodeView::Ssa,
    );
    assert!(tooltip.contains(&format!("SWAP16 %{}, %{}", usize::MAX, usize::MAX - 1)));
}

#[test]
fn ssa_detail_and_selected_pc_keep_aggregate_cards_as_source_summaries() {
    let mut report = with_receipts(vec![receipt(100, 0x80, "DUP1")]);
    let mut extra = report.cfg[2].clone();
    extra.id = 999;
    report.cfg.push(extra);
    let mut extra_ssa = report.ssa.blocks[2].clone();
    extra_ssa.state = 999;
    extra_ssa.instructions[0].results = vec![9101];
    report.ssa.blocks.push(extra_ssa);
    let (ctx, mut graph, _) = native_graph(&report);
    graph.set_mode(GraphMode::Blocks);
    let mut selection = Selection {
        value: None,
        state: Some(999),
        pc: Some(100),
    };
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    graph.zoom = 1.0;
    graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2();
    let measured = geometry(&graph);
    for display in [SsaDisplay::Compact, SsaDisplay::Full] {
        graph.set_ssa_display(display, selection);
        let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let lines = card_text(&output, &graph, canvas);
        assert!(lines.contains(&"Select an instance for SSA"));
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("%9101") && !line.contains("DUP1"))
        );
        assert_eq!(geometry(&graph), measured);
    }
}

fn outcome(state: usize, kind: OutcomeKind) -> Outcome {
    Outcome {
        state,
        kind,
        data: 0,
        store: 0,
    }
}

fn terminal_report() -> AnalysisReport {
    let mut report = crate::tests::report();
    report.edges.clear();
    report.outcomes = vec![
        outcome(0, OutcomeKind::Return),
        outcome(1, OutcomeKind::Revert),
        outcome(2, OutcomeKind::Failure),
    ];
    report
}

fn card<'a>(
    output: &'a FullOutput,
    graph: &Graph,
    canvas: Rect,
    state: usize,
) -> &'a egui::epaint::RectShape {
    let expected = graph.screen_rect(canvas, graph.scene.placement.nodes[&state]);
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.rect == expected => Some(rect),
            _ => None,
        })
        .expect("production graph must paint the measured card rectangle")
}

fn warning_dashes(output: &FullOutput, rect: Rect) -> usize {
    let outline = rect.shrink(3.0_f32.min(rect.width().min(rect.height()) * 0.2));
    output
        .shapes
        .iter()
        .filter(|shape| {
            matches!(&shape.shape, egui::Shape::LineSegment { points, stroke }
                if stroke.color == crate::palette::WARNING
                    && stroke.width == 1.0
                    && points.iter().all(|point| outline.contains(*point)))
        })
        .count()
}

#[test]
fn terminal_cards_paint_three_semantic_fills_and_selection_preserves_each_fill() {
    let report = terminal_report();
    let (ctx, mut graph, mut selection) = native_graph(&report);
    let measured = geometry(&graph);
    let expected = [
        (0, crate::palette::TERMINAL_NORMAL, "Root end: normal 1"),
        (1, crate::palette::TERMINAL_REVERT, "Root end: REVERT 1"),
        (
            2,
            crate::palette::TERMINAL_EXCEPTION,
            "Root end: exception 1",
        ),
    ];
    for (state, fill, label) in expected {
        for selected in [false, true] {
            selection.state = selected.then_some(state);
            let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
            let painted = card(&output, &graph, canvas, state);
            assert_eq!(painted.fill, fill);
            assert_eq!(
                painted.stroke.color,
                if selected {
                    crate::palette::ACCENT
                } else {
                    crate::palette::BORDER
                }
            );
            assert_eq!(painted.stroke.width, if selected { 1.5 } else { 1.0 });
            assert_eq!(warning_dashes(&output, painted.rect), 0);
            assert!(card_text(&output, &graph, canvas).contains(&label));
            assert_eq!(geometry(&graph), measured);
        }
    }
}

#[test]
fn coverage_and_frontier_warnings_have_independent_dashes_and_selection_outline() {
    let mut report = terminal_report();
    report.frontiers.push(Frontier {
        range: None,
        payload: None,
        from: Some(0),
        pc: Some(0),
        kind: FrontierKind::Memory,
        reason: FrontierDetails::Memory,
        detail: "unresolved memory".into(),
    });
    report.ssa.blocks[1].coverage = BlockCoverage::Stale;
    let (ctx, mut graph, mut selection) = native_graph(&report);
    for state in [0, 1] {
        selection.state = Some(state);
        let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
        let painted = card(&output, &graph, canvas, state);
        assert_eq!(painted.fill, crate::palette::PANEL);
        assert_eq!(painted.stroke.color, crate::palette::ACCENT);
        assert_eq!(painted.stroke.width, 1.5);
        assert!(warning_dashes(&output, painted.rect) > 4);
        assert!(graph.scene.nodes[&state].badges[0].0.contains("incomplete"));
    }
    // An unresolved state without any ending evidence stays neutral and never
    // gains a normal-end badge merely because no continuation was observed.
    report.outcomes.clear();
    graph.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    let painted = card(&output, &graph, canvas, 0);
    assert_eq!(painted.fill, crate::palette::PANEL);
    assert!(warning_dashes(&output, painted.rect) > 4);
    assert!(graph.scene.nodes[&0].badges.is_empty());
}

#[test]
fn mixed_aggregate_counts_and_may_terminate_cards_keep_a_neutral_fill() {
    let mut report = terminal_report();
    report.cfg[1].basic_block = Some(0);
    report.cfg[1].start_pc = report.cfg[0].start_pc;
    report.source_mut(1).instructions = report.source_mut(0).instructions.clone();
    let (ctx, mut graph, mut selection) = native_graph(&report);
    graph.set_mode(GraphMode::Blocks);
    frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    graph.fitted = false;
    graph.zoom = 1.0;
    graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2();
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    assert_eq!(card(&output, &graph, canvas, 0).fill, crate::palette::PANEL);
    let lines = card_text(&output, &graph, canvas);
    assert!(lines.contains(&"Mixed / incomplete termination"));
    assert!(lines.contains(&"Root end: normal 1 · REVERT 1"));
    assert!(lines.contains(&"Select an instance for SSA"));
    let node = &graph.scene.display.nodes[graph.scene.node_positions[&0]];
    let tooltip = super::super::content::node_tooltip(
        &(report).view(),
        graph.scene.index.as_ref().unwrap(),
        node,
        NodeView::Ssa,
    );
    assert!(tooltip.contains("Root end: normal 1 · REVERT 1 · exception 0"));

    report.edges.push(CfgEdge {
        id: 98,
        from: 2,
        to: 0,
        kind: EdgeKind::Fallthrough,
    });
    graph.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
    let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
    assert_eq!(card(&output, &graph, canvas, 2).fill, crate::palette::PANEL);
    assert!(card_text(&output, &graph, canvas).contains(&"May terminate · continuing 1"));
    assert!(card_text(&output, &graph, canvas).contains(&"Root end: exception 1"));
}

#[test]
fn frame_return_badges_low_zoom_and_ssa_detail_share_frozen_measured_geometry() {
    let mut report = terminal_report();
    report.outcomes.retain(|outcome| outcome.state != 2);
    report.cfg[2].frame_depth = 2;
    report.edges.push(CfgEdge {
        id: 98,
        from: 2,
        to: 0,
        kind: EdgeKind::Return,
    });
    let (ctx, mut graph, mut selection) = native_graph(&report);
    let measured = geometry(&graph);
    let topology = graph.scene.display.edges.clone();
    let preview_budget = graph.scene.nodes[&2].lines.len();
    for display in [SsaDisplay::Compact, SsaDisplay::Full] {
        graph.set_ssa_display(display, selection);
        for (zoom, detailed) in [(1.0, true), (0.4, false)] {
            graph.zoom = zoom;
            graph.pan = Vec2::splat(20.0) - graph.scene.placement.bounds.min.to_vec2() * zoom;
            let (output, canvas) = frame(&ctx, &mut graph, &report, &mut selection, vec![]);
            assert_eq!(
                card(&output, &graph, canvas, 2).fill,
                crate::palette::TERMINAL_NORMAL
            );
            let labels = card_text(&output, &graph, canvas);
            assert_eq!(labels.contains(&"Frame return: normal 1"), detailed);
            if !detailed {
                assert!(labels.contains(&"σ₂"));
                assert!(labels.contains(&"B₂"));
                assert!(labels.iter().all(|line| !line.contains("Root end")
                    && !line.contains("Frame return")
                    && !line.contains('%')));
            }
            assert_eq!(geometry(&graph), measured);
            assert_eq!(graph.scene.display.edges, topology);
            assert_eq!(graph.scene.nodes[&2].lines.len(), preview_budget);
            assert_eq!(graph.scene.nodes[&2].badges.len(), 1);
        }
    }
    let node = &graph.scene.display.nodes[graph.scene.node_positions[&2]];
    let tooltip = super::super::content::node_tooltip(
        &(report).view(),
        graph.scene.index.as_ref().unwrap(),
        node,
        NodeView::Ssa,
    );
    assert!(tooltip.contains("Frame return: normal 1 · REVERT 0 · exception 0"));
    assert!(tooltip.contains("Root end: normal 0 · REVERT 0 · exception 0"));
}
