use super::{Frontend, ReportTiming, Wait, backend_summary};
use crate::{
    Workspace,
    app::{JobOperation, Message, View, job::TaskPhase},
    tests::report,
};
use evm_abstract_protocol as api;

fn backend() -> api::AnalysisTimings {
    api::AnalysisTimings {
        queue_ns: 100_000_000,
        processing_ns: 900_000_000,
        validating_ns: 10_000_000,
        pinning_ns: 20_000_000,
        acquiring_ns: 30_000_000,
        analyzing_ns: 600_000_000,
        building_ssa_ns: 150_000_000,
        projecting_ns: 90_000_000,
    }
}

fn status(workspace: &mut Workspace, generation: u64, state: api::JobState) {
    workspace.receive_message(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Ok(api::JobReply {
            result: Ok(api::JobSnapshot {
                id: api::JobId(17),
                state,
                progress: api::AnalysisProgress::default(),
                timings: backend(),
            }),
        }),
    });
}

fn frame(workspace: &mut Workspace, size: egui::Vec2) -> Vec<String> {
    paint(size, |ui| {
        workspace.show(ui);
    })
}

fn paint(size: egui::Vec2, render: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let context = egui::Context::default();
    crate::palette::configure(&context);
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..egui::RawInput::default()
        },
        render,
    );
    output.textures_delta.clear();
    fn text(shape: &egui::Shape, strings: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(item) => strings.push(item.galley.text().into()),
            egui::Shape::Vec(items) => {
                for item in items {
                    text(item, strings);
                }
            }
            _ => {}
        }
    }
    let mut strings = Vec::new();
    for shape in output.shapes {
        text(&shape.shape, &mut strings);
    }
    strings
}

#[test]
fn waiting_uses_monotonic_elapsed_and_freezes_once_at_terminal_acknowledgment() {
    let mut wait = Wait::default();
    assert_eq!(wait.elapsed(10.0), None);
    wait.start(10.0);
    assert_eq!(wait.elapsed(12.5), Some(2_500_000_000));
    assert_eq!(wait.summary(12.5).as_deref(), Some("Elapsed: 2.50 s"));
    wait.finish(13.0);
    wait.finish(50.0);
    assert_eq!(wait.elapsed(100.0), Some(3_000_000_000));
    assert_eq!(wait.summary(100.0).as_deref(), Some("Total wait: 3.00 s"));
    wait.start(120.0);
    assert_eq!(wait.elapsed(121.0), Some(1_000_000_000));
}

#[test]
fn result_wait_includes_actual_first_visible_view_and_does_not_force_hidden_graphs() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.view = View::Ssa;
    let mut report = report();
    report.metadata.timings = backend();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    assert!(
        workspace.task.wait.running(),
        "JSON receipt is not view readiness"
    );
    assert!(workspace.report_timing.as_ref().unwrap().pending);
    assert!(
        workspace
            .accessible_status()
            .starts_with("Preparing result view:")
    );
    assert!(!workspace.graph.prepared());
    frame(&mut workspace, egui::vec2(420.0, 650.0));
    let timing = workspace.report_timing.as_ref().unwrap().clone();
    assert!(!timing.pending);
    assert!(!workspace.task.wait.running());
    assert!(timing.frontend.view_ns > 0);
    assert!(
        !workspace.graph.prepared(),
        "hidden lazy CFG remains unbuilt"
    );
    workspace.view = View::Graph;
    frame(&mut workspace, egui::vec2(1440.0, 1000.0));
    let after = workspace.report_timing.as_ref().unwrap();
    assert!(workspace.graph.prepared());
    assert_eq!(
        after.frontend, timing.frontend,
        "later graph work is not first-view work"
    );
    assert_eq!(after.wait, timing.wait);
    let status = workspace.accessible_status();
    for label in [
        "Total wait:",
        "Backend processing: 0.90 s",
        "Queue: 0.10 s",
        "RPC: 0.05 s",
        "Abstract analysis: 0.60 s",
        "SSA construction: 0.15 s",
        "Report projection: 0.09 s",
        "Frontend preparation:",
        "First view setup:",
    ] {
        assert!(status.contains(label), "missing {label}: {status}");
    }
}

#[test]
fn pending_or_discarded_view_work_is_accumulated_without_finishing_wait() {
    let mut wait = Wait::default();
    wait.start(1.0);
    let mut timing = ReportTiming {
        generation: 1,
        wait,
        frontend: Frontend::default(),
        pending: true,
    };
    timing.record_view(40_000_000, false, 5.0);
    assert!(timing.wait.running());
    timing.record_view(60_000_000, true, 6.0);
    assert_eq!(timing.frontend.view_ns, 100_000_000);
    assert_eq!(timing.wait.elapsed(20.0), Some(5_000_000_000));
    timing.record_view(800_000_000, true, 20.0);
    assert_eq!(timing.frontend.view_ns, 100_000_000);
}

#[test]
fn disconnected_polling_keeps_elapsed_live_while_terminal_failure_and_cancel_freeze_it() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    status(&mut workspace, 1, api::JobState::Running);
    workspace.receive_message(Message::Status {
        generation: 1,
        operation: JobOperation::Poll,
        result: Err(crate::TransportError::Timeout { seconds: 15 }),
    });
    assert!(workspace.task.wait.running());
    assert!(workspace.accessible_status().contains("Elapsed:"));
    status(&mut workspace, 1, api::JobState::Cancelled);
    let frozen = workspace.task.wait.clone();
    assert!(!frozen.running());
    status(&mut workspace, 1, api::JobState::Running);
    assert_eq!(workspace.task.wait, frozen);
    assert!(workspace.accessible_status().contains("Total wait:"));
    workspace.initial_command();
    let mut invalid = report();
    invalid.schema_version = 0;
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(invalid),
    }));
    assert_eq!(workspace.task.phase, TaskPhase::Failed);
    assert!(!workspace.task.wait.running());
    assert!(workspace.accessible_status().contains("Total wait:"));
}

#[test]
fn result_retries_accumulate_client_work_and_stale_callbacks_cannot_relabel_previous_report() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    status(&mut workspace, 1, api::JobState::Completed);
    workspace.receive_message(Message::Result {
        generation: 1,
        id: api::JobId(17),
        result: Box::new(Err(crate::TransportError::Timeout { seconds: 60 })),
        transfer_ns: 70_000_000,
        parsing_ns: 0,
    });
    assert!(workspace.task.wait.running());
    let mut report = report();
    report.metadata.timings = backend();
    workspace.receive_message(Message::Result {
        generation: 1,
        id: api::JobId(17),
        result: Box::new(Ok(api::AnalyzeReply { result: Ok(report) })),
        transfer_ns: 30_000_000,
        parsing_ns: 10_000_000,
    });
    frame(&mut workspace, egui::vec2(1440.0, 1000.0));
    let previous = workspace.report_timing.as_ref().unwrap().clone();
    assert_eq!(previous.frontend.transfer_ns, 100_000_000);
    assert_eq!(previous.frontend.parsing_ns, 10_000_000);
    workspace.initial_command();
    assert!(workspace.task.wait.running());
    assert_eq!(workspace.task.frontend, Frontend::default());
    workspace.receive_message(Message::Result {
        generation: 1,
        id: api::JobId(17),
        result: Box::new(Ok(api::AnalyzeReply {
            result: Ok(crate::tests::empty_report()),
        })),
        transfer_ns: u64::MAX,
        parsing_ns: u64::MAX,
    });
    assert_eq!(workspace.task.frontend, Frontend::default());
    assert_eq!(
        workspace.report_timing.as_ref().unwrap().frontend,
        previous.frontend
    );
    let summary = workspace.accessible_status();
    assert!(summary.contains("Elapsed:"));
    assert!(summary.contains("Previous report timing; Backend processing: 0.90 s"));
    assert!(summary.contains("Previous report Total wait:"));
}

#[test]
fn narrow_status_keeps_the_elapsed_clock_visible_and_backend_stages_are_authoritative() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    let mut snapshot = api::JobSnapshot {
        id: api::JobId(17),
        state: api::JobState::Running,
        progress: api::AnalysisProgress::default(),
        timings: backend(),
    };
    snapshot.progress.phase = api::AnalysisPhase::Analyzing;
    workspace.receive_message(Message::Status {
        generation: 1,
        operation: JobOperation::Submit,
        result: Ok(api::JobReply {
            result: Ok(snapshot),
        }),
    });
    let strings = frame(&mut workspace, egui::vec2(360.0, 420.0));
    assert!(
        strings.iter().any(|text| text.starts_with("Elapsed:")),
        "{strings:?}"
    );
    assert!(
        backend_summary(&workspace.task.snapshot.as_ref().unwrap().timings)
            .contains("Abstract analysis: 0.60 s")
    );
    // Client phase arrival neither starts nor overwrites the authoritative
    // stage duration: the payload's 600 ms remains the value displayed.
    let summary = workspace.accessible_status();
    assert!(summary.contains("Abstract analysis: 0.60 s"));
}

#[test]
fn completed_timing_inspector_keeps_all_backend_and_frontend_phases_readable() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    let mut report = report();
    report.metadata.timings = backend();
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    workspace.inspector.tab = crate::app::inspector::Tab::Timing;
    frame(&mut workspace, egui::vec2(1440.0, 1600.0));
    workspace.inspector.timings = workspace.timing_details();
    let visible = paint(egui::vec2(1440.0, 1600.0), |ui| {
        workspace.inspector.show(
            ui,
            workspace.report.as_ref().unwrap(),
            &mut workspace.selection,
            None,
        );
    });
    for label in [
        "Report timing",
        "Backend processing",
        "Queue",
        "Input validation",
        "RPC · combined",
        "Snapshot pinning",
        "RPC acquisition",
        "Abstract analysis",
        "SSA construction",
        "Report projection",
        "Frontend preparation",
        "Result transfer",
        "JSON parsing",
        "First view setup",
    ] {
        assert!(
            visible.iter().any(|text| text == label),
            "missing {label}: {visible:?}"
        );
    }
}
