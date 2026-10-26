use super::{Graph, Selection};
use crate::app::model::ReportViewId;
use egui::{Context, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisInput, AnalysisNamespace, AnalysisReport, AnalyzeRequest, BytecodeInput,
};

fn report() -> AnalysisReport {
    let mut request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "6003565b6007565b00".into(),
            address: None,
        }),
        ..Default::default()
    };
    request.limits.root_terminal_coverage = false;
    evm_abstract_server::analyze::analyze(request).unwrap()
}

fn frame(
    ctx: &Context,
    graph: &mut Graph,
    report: &AnalysisReport,
    selection: &mut Selection,
) -> FullOutput {
    let identity = ReportViewId::fixture(1, AnalysisNamespace::Baseline).minimum(None);
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 640.0))),
            ..Default::default()
        },
        |ui| {
            let _ = graph.show_minimum_scoped(ui, report, identity, selection);
        },
    );
    output.textures_delta.clear();
    output
}

#[test]
fn actual_scoped_source_layout_is_readable_and_low_zoom_omits_card_text() {
    let report = report();
    let model = report.minimum.as_deref().unwrap();
    let mut selection = Selection::default();
    selection.select_summary(model, crate::app::minimum::initial_summary(model));
    let mut graph = Graph::default();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    frame(&ctx, &mut graph, &report, &mut selection);
    let scene = graph.scene.minimum.as_ref().unwrap();
    assert!(scene.error.is_none(), "{:?}", scene.error);
    assert!(graph.zoom >= 0.9);
    assert!(
        scene
            .text
            .iter()
            .any(|card| card.title.starts_with("Block 0x0000"))
    );
    let output = frame(&ctx, &mut graph, &report, &mut selection);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().starts_with("Block 0x0000"))));
    graph.restore_camera(0.005, Vec2::ZERO);
    let output = frame(&ctx, &mut graph, &report, &mut selection);
    let titles: Vec<_> = graph
        .scene
        .minimum
        .as_ref()
        .unwrap()
        .text
        .iter()
        .map(|card| card.title.as_str())
        .collect();
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if titles.contains(&text.galley.text()))));
}

#[test]
fn canonical_history_camera_survives_the_first_scoped_layout() {
    let report = report();
    let mut selection = Selection::default();
    let model = report.minimum.as_deref().unwrap();
    selection.select_summary(model, crate::app::minimum::initial_summary(model));
    let identity = ReportViewId::fixture(1, AnalysisNamespace::Baseline).minimum(None);
    let mut graph = Graph::default();
    graph.activate(identity);
    assert!(graph.restore_camera(1.2, Vec2::new(-120.0, 44.0)));
    let camera = graph.camera();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    frame(&ctx, &mut graph, &report, &mut selection);
    assert_eq!(graph.camera(), camera);
    assert!(graph.prepared());
    assert!(!graph.camera_restored);
}

mod interaction;
