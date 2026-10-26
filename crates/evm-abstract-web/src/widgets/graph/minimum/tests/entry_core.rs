//! Whole workbench geometry, using full (unclipped) cached card rectangles.
//! This semantic fixture protects layout policy; large captured inputs have a
//! separate source-qualified offGit regression, not a shortened benchmark.
use crate::{Workspace, app::Command};
use egui::{Context, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{AnalysisInput, AnalyzeReply, AnalyzeRequest, BytecodeInput};

#[test]
fn entry_and_both_real_branches_fit_the_actual_workbench_canvas_at_readable_scale() {
    let request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "3615600957600150005b00".into(),
            address: None,
        }),
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    for size in [
        Vec2::new(1440.0, 1000.0),
        Vec2::new(320.0, 480.0),
        Vec2::new(390.0, 844.0),
    ] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut workspace = Workspace::default();
        assert!(matches!(
            workspace.initial_command(),
            Command::Submit { .. }
        ));
        workspace.receive(Ok(AnalyzeReply {
            result: Ok(report.clone()),
        }));
        for _ in 0..8 {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    let _ = workspace.show(ui);
                },
            );
            output.textures_delta.clear();
            if workspace.graph.prepared() && workspace.graph.painted_core_nodes().len() == 3 {
                break;
            }
        }
        let canvas = workspace
            .graph
            .painted_canvas
            .expect("actual canvas painted");
        let core = workspace.graph.painted_core_nodes();
        let painted = workspace.graph.painted_nodes();
        assert!(
            core.iter().all(|card| painted.contains(card)),
            "core geometry must be actual painted node geometry"
        );
        assert_eq!(
            core.len(),
            3,
            "actual entry plus both direct source choices"
        );
        assert!(
            workspace.graph.camera().0 >= 1.0,
            "do not fit text to a tiny scale"
        );
        for (title, rect) in core {
            assert!(
                canvas.expand(0.5).contains_rect(rect),
                "{size:?}: unclipped {title} {rect:?} outside actual {canvas:?}"
            );
            assert!(rect.width() >= 88.0 && rect.height() >= 64.0);
        }
    }
}

#[test]
fn explicit_model_overview_has_full_model_policy_and_is_not_full_source_cfg() {
    use super::super::control::GraphWindow;
    use crate::widgets::graph::{Graph, GraphScope};
    let request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "3615600957600150005b00".into(),
            address: None,
        }),
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    let summary = crate::app::minimum::initial_summary(report.minimum.as_deref().unwrap());
    let mut graph = Graph::default();
    graph.ensure_minimum_scope(
        &report,
        GraphScope::Overview { summary },
        Some(GraphWindow::Entry),
    );
    let scene = graph.scene.minimum.as_ref().unwrap();
    assert_eq!(scene.window, GraphWindow::FullSource);
    assert_eq!(
        scene.control.level,
        super::super::control::GraphLevel::Model
    );
    graph.ensure_minimum_scope(
        &report,
        GraphScope::Entry { summary },
        Some(GraphWindow::FullSource),
    );
    let scene = graph.scene.minimum.as_ref().unwrap();
    assert_eq!(scene.window, GraphWindow::FullSource);
    assert_eq!(
        scene.control.level,
        super::super::control::GraphLevel::Source
    );
}
