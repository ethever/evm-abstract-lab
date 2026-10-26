use super::*;
use crate::app::{Command, JobOperation, Message, View, Workspace};
use evm_abstract_protocol as api;

fn reports() -> (AnalysisReport, AnalysisReport) {
    reports_for("600160020100")
}

fn reports_for(code: &str) -> (AnalysisReport, AnalysisReport) {
    let complete = evm_abstract_server::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: Some(format!("0x{}0101", "00".repeat(18))),
        }),
        ..Default::default()
    })
    .unwrap();
    complete.validate().unwrap();
    assert!(complete.minimum.is_some() && complete.native.is_some());
    // A consumer profile over the producer's actual immutable model. Only
    // optional graphs/receipts are omitted; no input, control, numerical bound
    // or source/revision qualification is reconstructed. HTTP/GPU acceptance
    // separately proves that production publishes this profile before Native.
    let mut minimal = complete.clone();
    minimal.global = None;
    minimal.native = None;
    minimal.selected_view = api::AnalysisNamespace::Baseline;
    minimal.rich_export = api::RichExportOutcome::NotRequested;
    minimal.hybrid.as_mut().unwrap().local_prefixes.clear();
    minimal.validate().unwrap();
    (minimal, complete)
}

#[test]
fn body_and_interior_selection_navigate_exact_may_source_without_native_ids() {
    let (report, _) = reports_for("6003565b6007565b00");
    let model = report.minimum.as_ref().unwrap();
    let summary = initial_summary(model);
    let cfg = &model.summaries[summary].relation.regional_cfg;
    let boundary = cfg
        .nodes
        .iter()
        .find(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == api::ProcessLocation::Block(2))
        })
        .unwrap()
        .id;
    let interior = cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer
                .as_ref()
                .and_then(|t| t.process.as_ref())
                .is_some_and(|p| p.control.is_some())
        })
        .unwrap()
        .id;
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    workspace.minimum_selection.select_node(
        workspace.report.as_ref().unwrap().minimum.as_ref().unwrap(),
        boundary,
    );
    workspace.view = View::Disassembly;
    let ctx = context();
    let size = egui::vec2(1440.0, 1000.0);
    let output = render(&ctx, &mut workspace, size);
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text == "MAY source: block index 2")
    );
    let model = workspace.report.as_ref().unwrap().minimum.as_ref().unwrap();
    workspace.minimum_selection.select_node(model, interior);
    workspace.minimum_selection.control = Some(api::ProcessLocation::Block(1));
    let output = render(&ctx, &mut workspace, size);
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text == "MAY source: block index 1")
    );
    assert_eq!(workspace.selection, crate::app::Selection::default());
    assert_eq!(
        workspace.minimum_source.focus.unwrap().2,
        api::ProcessLocation::Block(1)
    );
    workspace.inspector.tab = super::super::inspector::Tab::Coverage;
    click(&ctx, &mut workspace, size, "Maximize");
    let output = render(&ctx, &mut workspace, size);
    let text: Vec<_> = crate::tests::painted_text(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert!(
        text.iter()
            .any(|text| text.contains("Selected MAY source: block index 1")),
        "{text:?}"
    );
    assert!(text.iter().any(|text| text.contains("Owner class:")));
    assert!(text.iter().any(|text| text.contains("MAY source edge:")));
}

#[test]
fn clicking_an_interior_after_an_original_account_keeps_its_actual_source() {
    let (report, _) = reports_for("6003565b6007565b00");
    let ctx = context();
    let size = egui::vec2(1440.0, 1000.0);
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    let program = workspace.program.unwrap();
    workspace.minimum_selection.account = Some(0);
    workspace.view = View::Graph;
    click(
        &ctx,
        &mut workspace,
        size,
        &format!("MAY B1 · program {program} · source 0x0003"),
    );
    assert_eq!(
        workspace.minimum_selection.control,
        Some(api::ProcessLocation::Block(1))
    );
    assert_eq!(workspace.minimum_selection.account, None);
    assert_eq!(workspace.program, Some(program));
    assert_eq!(workspace.selection, crate::app::Selection::default());
}

fn frame(
    ctx: &egui::Context,
    workspace: &mut Workspace,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| {
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    output
}
fn render(ctx: &egui::Context, workspace: &mut Workspace, size: egui::Vec2) -> egui::FullOutput {
    let mut output = frame(ctx, workspace, size, vec![]);
    for _ in 0..3 {
        output = frame(ctx, workspace, size, vec![]);
    }
    output
}
fn click(ctx: &egui::Context, workspace: &mut Workspace, size: egui::Vec2, label: &str) {
    let output = render(ctx, workspace, size);
    let pos = crate::tests::painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("missing {label}: {:?}", crate::tests::painted_text(&output)))
        .1
        + egui::vec2(5.0, 5.0);
    for pressed in [true, false] {
        frame(
            ctx,
            workspace,
            size,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}
fn context() -> egui::Context {
    let context = egui::Context::default();
    crate::palette::configure(&context);
    context
}
fn publication(
    version: u64,
    minimum: u64,
    global: Option<u64>,
    native: Option<u64>,
) -> api::JobPublication {
    let sequence = |value| api::PublicationVersion(api::DecimalU64(value));
    api::JobPublication {
        version: sequence(version),
        minimum_version: Some(sequence(minimum)),
        global_version: global.map(sequence),
        native_version: native.map(sequence),
        model_closed: true,
    }
}
fn deliver(
    workspace: &mut Workspace,
    state: api::JobState,
    publication: api::JobPublication,
    report: AnalysisReport,
) {
    let generation = workspace.task.generation();
    let id = api::JobId(17);
    workspace.receive_message(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Ok(api::JobReply {
            result: Ok(api::JobSnapshot {
                id,
                state,
                publication: Some(publication.clone()),
                progress: Default::default(),
                timings: Default::default(),
            }),
        })),
    });
    assert!(matches!(
        workspace.task.next_command(10.0),
        Some(Command::Result { .. })
    ));
    workspace.receive_message(Message::Result {
        generation,
        id,
        result: Box::new(Ok(api::JobResultReply {
            result: Ok(api::JobResult {
                id,
                publication,
                report,
            }),
        })),
        transfer_ns: 1,
        parsing_ns: 1,
    });
    assert!(workspace.task.error.is_none(), "{:?}", workspace.task.error);
}

#[test]
fn minimum_only_renders_all_views_and_mobile_directory_without_private_state_or_ssa() {
    let (report, _) = reports();
    let ctx = context();
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    for size in [
        egui::vec2(1440.0, 1000.0),
        egui::vec2(390.0, 844.0),
        egui::vec2(1440.0, 1000.0),
    ] {
        for view in [View::Split, View::Disassembly, View::Graph, View::Ssa] {
            workspace.view = view;
            workspace.directory.open = true;
            let output = render(&ctx, &mut workspace, size);
            assert!(!output.shapes.is_empty());
            let status = workspace.accessible_status();
            assert!(status.contains("Global model Converged"), "{status}");
            assert!(workspace.report_view.identity().is_minimum());
            assert_eq!(workspace.selection, crate::app::Selection::default());
            if view == View::Ssa {
                assert!(crate::tests::painted_text(&output).iter().any(|(text, _)| {
                    text.contains("Instruction SSA is available in Detailed paths")
                }));
            }
        }
    }
    assert!(!workspace.report_timing.as_ref().unwrap().pending);
}

#[test]
fn native_and_rich_arrival_keep_the_canonical_minimum_camera_selection_and_source_cache() {
    let (first, rich) = reports_for("6003565b6007565b00");
    let ctx = context();
    let size = egui::vec2(1440.0, 1000.0);
    let mut workspace = Workspace::default();
    workspace.initial_command();
    deliver(
        &mut workspace,
        api::JobState::Running,
        publication(1, 1, None, None),
        first,
    );
    render(&ctx, &mut workspace, size);
    workspace.minimum_selection.control = Some(api::ProcessLocation::Block(1));
    render(&ctx, &mut workspace, size);
    workspace.graph.zoom_at(egui::vec2(80.0, 100.0), 0.7);
    let selected = workspace.minimum_selection;
    let identity = workspace.report_view.identity();
    let camera = workspace.graph.camera();
    workspace.view = View::Disassembly;
    let before_source = render(&ctx, &mut workspace, size);
    let cache = crate::tests::painted_text(&before_source)
        .into_iter()
        .find(|(text, _)| text == "Block 0x0003")
        .expect("the selected MAY source row is actually rendered");

    let global = rich.global.as_ref().map(|_| 2);
    let native = rich.native.as_ref().map(|_| 2);
    deliver(
        &mut workspace,
        api::JobState::Completed,
        publication(2, 1, global, native),
        rich,
    );
    let after_source = render(&ctx, &mut workspace, size);
    assert_eq!(workspace.report_view.identity(), identity);
    assert_eq!(workspace.minimum_selection, selected);
    assert_eq!(workspace.graph.camera(), camera);
    assert_eq!(
        crate::tests::painted_text(&after_source)
            .into_iter()
            .find(|(text, _)| text == "Block 0x0003")
            .expect("the same source row remains visible after richer publication"),
        cache
    );

    workspace.activate_view(crate::app::model::Activation::Namespace(
        api::AnalysisNamespace::Native,
    ));
    render(&ctx, &mut workspace, size);
    assert!(!workspace.report_view.identity().is_minimum());
    assert!(workspace.selection.state.is_some());
    assert_eq!(workspace.selection.value, None);
    workspace.activate_view(crate::app::model::Activation::Namespace(
        api::AnalysisNamespace::Baseline,
    ));
    render(&ctx, &mut workspace, size);
    assert!(workspace.report_view.identity().is_minimum());
    assert_eq!(workspace.selection, crate::app::Selection::default());
    assert_eq!(workspace.graph.camera(), camera);
}

#[test]
fn refused_and_cancelled_details_keep_the_same_usable_minimum_and_joint_bounds() {
    let (first, _) = reports();
    let ctx = context();
    let size = egui::vec2(1440.0, 1000.0);
    let mut workspace = Workspace::default();
    workspace.initial_command();
    deliver(
        &mut workspace,
        api::JobState::Running,
        publication(1, 1, None, None),
        first.clone(),
    );
    render(&ctx, &mut workspace, size);
    workspace.graph.zoom_at(egui::vec2(80.0, 100.0), 0.7);
    let identity = workspace.report_view.identity();
    let camera = workspace.graph.camera();
    let retained =
        std::sync::Arc::clone(workspace.report.as_ref().unwrap().minimum.as_ref().unwrap());
    let mut refused = first.clone();
    refused.rich_export = api::RichExportOutcome::Refused(api::ApiError {
        code: api::ApiErrorCode::Export,
        message: "optional rich presentation refused".into(),
        details: api::ErrorDetails::Worker(api::WorkerFailure {
            kind: api::WorkerErrorKind::Protocol,
            os_code: None,
        }),
    });
    deliver(
        &mut workspace,
        api::JobState::Running,
        publication(2, 1, None, None),
        refused,
    );
    let output = render(&ctx, &mut workspace, size);
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains("presentation refused"))
    );
    assert!(workspace.task.cancellable());
    assert_eq!(workspace.report_view.identity(), identity);
    assert_eq!(workspace.graph.camera(), camera);
    let mut cancelled = first;
    cancelled.rich_export = api::RichExportOutcome::Cancelled;
    deliver(
        &mut workspace,
        api::JobState::Cancelled,
        publication(3, 1, None, None),
        cancelled,
    );
    render(&ctx, &mut workspace, size);
    assert_eq!(workspace.report_view.identity(), identity);
    assert_eq!(workspace.graph.camera(), camera);
    assert!(std::sync::Arc::ptr_eq(
        &retained,
        workspace.report.as_ref().unwrap().minimum.as_ref().unwrap()
    ));
    workspace.inspector.tab = super::super::inspector::Tab::Outcomes;
    workspace.inspector.expanded = true;
    click(&ctx, &mut workspace, size, "Maximize");
    let output = render(&ctx, &mut workspace, size);
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains("same relative outcome")),
        "{:?}",
        crate::tests::painted_text(&output)
    );
}

#[test]
fn minimum_inspector_shared_controls_maximize_restore_and_original_input_values() {
    let (report, _) = reports();
    let ctx = context();
    let size = egui::vec2(1440.0, 1000.0);
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    click(&ctx, &mut workspace, size, "Maximize");
    assert!(workspace.inspector.maximized());
    click(
        &ctx,
        &mut workspace,
        size,
        "Original accounts and root template",
    );
    let output = render(&ctx, &mut workspace, size);
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains("Physical account"))
    );
    click(&ctx, &mut workspace, size, "Restore");
    assert!(!workspace.inspector.maximized());
    assert_eq!(workspace.selection, crate::app::Selection::default());
}
