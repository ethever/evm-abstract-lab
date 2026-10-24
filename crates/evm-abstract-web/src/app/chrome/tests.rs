use crate::{
    Workspace,
    app::{Command, JobOperation, Message, View, job::TaskPhase},
    tests::report,
};
use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, epaint::TextShape};
use evm_abstract_protocol as api;

fn workspace() -> Workspace {
    Workspace {
        view: View::Ssa,
        ..Workspace::default()
    }
}

fn start(workspace: &mut Workspace) -> u64 {
    let Command::Submit { generation, .. } = workspace.initial_command() else {
        panic!("expected submission");
    };
    generation
}

fn status(
    workspace: &mut Workspace,
    generation: u64,
    id: api::JobId,
    operation: JobOperation,
    state: api::JobState,
) {
    workspace.receive_message(Message::Status {
        generation,
        operation,
        result: Ok(api::JobReply {
            result: Ok(api::JobSnapshot {
                id,
                state,
                progress: api::AnalysisProgress::default(),
                timings: api::AnalysisTimings::default(),
            }),
        }),
    });
}

fn result(generation: u64, id: api::JobId) -> Message {
    let mut report = report();
    report.metadata.work = generation;
    Message::Result {
        generation,
        id,
        result: Box::new(Ok(api::AnalyzeReply { result: Ok(report) })),
        transfer_ns: 0,
        parsing_ns: 0,
    }
}

fn complete(workspace: &mut Workspace, generation: u64, id: api::JobId) {
    status(
        workspace,
        generation,
        id,
        JobOperation::Submit,
        api::JobState::Running,
    );
    status(
        workspace,
        generation,
        id,
        JobOperation::Poll,
        api::JobState::Completed,
    );
    workspace.receive_message(result(generation, id));
    assert_eq!(workspace.task.phase, TaskPhase::Ready);
    assert_eq!(workspace.report_job_id, Some(id));
}

fn frame(
    context: &Context,
    workspace: &mut Workspace,
    time: &mut f64,
    width: f32,
    events: Vec<Event>,
) -> FullOutput {
    *time += 0.1;
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 800.0))),
        time: Some(*time),
        events,
        ..RawInput::default()
    };
    workspace.prepare_input(&mut input);
    let mut output = context.run_ui(input, |ui| {
        workspace.show(ui);
    });
    output.textures_delta.clear();
    output
}

fn painted(output: &FullOutput) -> Vec<(Rect, TextShape)> {
    fn collect(shape: &egui::Shape, clip: Rect, text: &mut Vec<(Rect, TextShape)>) {
        match shape {
            egui::Shape::Text(item) => text.push((clip, item.clone())),
            egui::Shape::Vec(items) => {
                for item in items {
                    collect(item, clip, text);
                }
            }
            _ => {}
        }
    }
    let mut text = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut text);
    }
    text
}

fn assert_copy(
    workspace: &mut Workspace,
    width: f32,
    label: &str,
    id: api::JobId,
    button_index: usize,
) {
    let context = Context::default();
    crate::palette::configure(&context);
    let mut time = 0.0;
    // Render the actual workbench through first-view preparation before
    // checking the terminal status and discovering the rendered Copy button.
    frame(&context, workspace, &mut time, width, vec![]);
    let output = frame(&context, workspace, &mut time, width, vec![]);
    let text = painted(&output);
    let expected = format!("{label}: {}", id.0);
    assert!(workspace.accessible_status().contains(&expected));
    let (clip, identity) = text
        .iter()
        .find(|(_, item)| item.galley.text() == expected)
        .expect("full labelled Job ID is rendered");
    let glyphs: String = identity
        .galley
        .rows
        .iter()
        .flat_map(|row| &row.glyphs)
        .map(|glyph| glyph.chr)
        .collect();
    assert_eq!(glyphs, expected, "all decimal digits must be painted");
    let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 800.0));
    let identity_rect = Rect::from_min_size(identity.pos, identity.galley.size());
    assert!(clip.contains_rect(identity_rect) && screen.contains_rect(identity_rect));
    let (clip, button) = text
        .iter()
        .filter(|(_, item)| item.galley.text() == "Copy")
        .nth(button_index)
        .expect("rendered Copy control");
    let button_rect = Rect::from_min_size(button.pos, button.galley.size());
    assert!(clip.contains_rect(button_rect) && screen.contains_rect(button_rect));
    let position = button_rect.center();
    let mut copied = Vec::new();
    for pressed in [true, false] {
        let output = frame(
            &context,
            workspace,
            &mut time,
            width,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        for command in output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                copied.push(text);
            }
        }
    }
    assert_eq!(copied, vec![id.0.to_string()]);
}

#[test]
fn completed_job_id_stays_exact_visible_and_copyable_after_view_preparation() {
    for raw in [9_007_199_254_740_993, 7_086_237_777_096_729_000, u64::MAX] {
        let id = api::JobId(raw);
        let mut workspace = workspace();
        let generation = start(&mut workspace);
        complete(&mut workspace, generation, id);
        for width in [390.0, 1200.0] {
            assert_copy(&mut workspace, width, "Job ID", id, 0);
            assert!(workspace.accessible_status().starts_with("Ready:"));
            assert_eq!(workspace.task.snapshot.as_ref().unwrap().id, id);
        }
    }
}

#[test]
fn failed_and_cancelled_job_ids_remain_copyable_with_or_without_a_previous_report() {
    let old = api::JobId(9_007_199_254_740_993);
    let current = api::JobId(u64::MAX);
    for previous in [false, true] {
        for cancelled in [false, true] {
            let mut workspace = workspace();
            if previous {
                let generation = start(&mut workspace);
                complete(&mut workspace, generation, old);
            }
            let generation = start(&mut workspace);
            status(
                &mut workspace,
                generation,
                current,
                JobOperation::Submit,
                api::JobState::Running,
            );
            let state = if cancelled {
                assert!(
                    matches!(workspace.task.cancel(), Some(Command::Cancel { id, .. }) if id == current)
                );
                api::JobState::Cancelled
            } else {
                api::JobState::Failed(api::ApiError {
                    code: api::ApiErrorCode::WorkerFailed,
                    message: "Worker exited".into(),
                    details: api::ErrorDetails::Worker(api::WorkerFailure {
                        kind: api::WorkerErrorKind::Exit,
                        os_code: None,
                    }),
                })
            };
            status(
                &mut workspace,
                generation,
                current,
                JobOperation::Poll,
                state,
            );
            assert_eq!(
                workspace.task.phase,
                if cancelled {
                    TaskPhase::Cancelled
                } else {
                    TaskPhase::Failed
                }
            );
            assert_eq!(workspace.previous_report, previous);
            assert_copy(
                &mut workspace,
                390.0,
                if previous { "Current Job ID" } else { "Job ID" },
                current,
                0,
            );
            if previous {
                assert_eq!(workspace.report_job_id, Some(old));
                assert_eq!(
                    workspace.report.as_ref().unwrap().metadata.work,
                    generation - 1
                );
                assert_copy(&mut workspace, 390.0, "Result Job ID", old, 1);
            }
        }
    }
}

#[test]
fn retained_result_identity_changes_only_for_the_accepted_new_result() {
    let old = api::JobId(9_007_199_254_740_993);
    let current = api::JobId(u64::MAX);
    let mut workspace = workspace();
    let first = start(&mut workspace);
    complete(&mut workspace, first, old);
    let second = start(&mut workspace);
    assert!(workspace.previous_report);
    assert!(workspace.task.snapshot.is_none());
    assert_copy(&mut workspace, 390.0, "Result Job ID", old, 0);
    status(
        &mut workspace,
        second,
        current,
        JobOperation::Submit,
        api::JobState::Running,
    );
    assert_copy(&mut workspace, 390.0, "Current Job ID", current, 0);
    assert_copy(&mut workspace, 390.0, "Result Job ID", old, 1);
    status(
        &mut workspace,
        first,
        old,
        JobOperation::Poll,
        api::JobState::Completed,
    );
    workspace.receive_message(result(first, old));
    assert_eq!(workspace.task.snapshot.as_ref().unwrap().id, current);
    status(
        &mut workspace,
        second,
        current,
        JobOperation::Poll,
        api::JobState::Completed,
    );
    workspace.receive_message(result(second, old));
    workspace.receive_message(result(first, current));
    assert_eq!(workspace.task.phase, TaskPhase::Fetching);
    assert_eq!(workspace.report_job_id, Some(old));
    assert_eq!(workspace.report.as_ref().unwrap().metadata.work, first);
    assert_copy(&mut workspace, 390.0, "Result Job ID", old, 1);
    workspace.receive_message(Message::Result {
        generation: second,
        id: current,
        result: Box::new(Err(crate::TransportError::Timeout { seconds: 60 })),
        transfer_ns: 0,
        parsing_ns: 0,
    });
    assert_eq!(workspace.report_job_id, Some(old));
    workspace.receive_message(result(second, current));
    assert!(!workspace.previous_report);
    assert_eq!(workspace.report_job_id, Some(current));
    assert_eq!(workspace.report.as_ref().unwrap().metadata.work, second);
    assert_copy(&mut workspace, 390.0, "Job ID", current, 0);
    assert!(!workspace.accessible_status().contains("Result Job ID:"));
    workspace.receive_message(result(first, old));
    assert_eq!(workspace.report_job_id, Some(current));
    assert_eq!(workspace.report.as_ref().unwrap().metadata.work, second);
    assert_copy(&mut workspace, 390.0, "Job ID", current, 0);
}

#[test]
fn invalid_result_cannot_bind_the_visible_report_to_the_new_job() {
    let old = api::JobId(9_007_199_254_740_993);
    let current = api::JobId(u64::MAX);
    let mut workspace = workspace();
    let first = start(&mut workspace);
    complete(&mut workspace, first, old);
    let second = start(&mut workspace);
    status(
        &mut workspace,
        second,
        current,
        JobOperation::Submit,
        api::JobState::Completed,
    );
    let mut invalid = report();
    invalid.schema_version = 0;
    workspace.receive_message(Message::Result {
        generation: second,
        id: current,
        result: Box::new(Ok(api::AnalyzeReply {
            result: Ok(invalid),
        })),
        transfer_ns: 0,
        parsing_ns: 0,
    });
    assert_eq!(workspace.task.phase, TaskPhase::Failed);
    assert_eq!(workspace.report_job_id, Some(old));
    assert_eq!(workspace.report.as_ref().unwrap().metadata.work, first);
    assert!(workspace.previous_report);
    assert_copy(&mut workspace, 390.0, "Current Job ID", current, 0);
    assert_copy(&mut workspace, 390.0, "Result Job ID", old, 1);
}

#[test]
fn standalone_receive_does_not_inherit_the_previous_server_result_id() {
    let id = api::JobId(u64::MAX);
    let mut workspace = workspace();
    let generation = start(&mut workspace);
    complete(&mut workspace, generation, id);
    workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(report()),
    }));
    assert!(workspace.report_job_id.is_none());
    assert_copy(&mut workspace, 390.0, "Current Job ID", id, 0);
    let status = workspace.accessible_status();
    assert!(status.contains("Result Job ID: unavailable"));
    assert!(!status.contains(&format!("Result Job ID: {}", id.0)));
    let context = Context::default();
    crate::palette::configure(&context);
    let mut time = 0.0;
    let output = frame(&context, &mut workspace, &mut time, 390.0, vec![]);
    assert!(
        painted(&output)
            .iter()
            .any(|(_, item)| item.galley.text() == "Result Job ID: unavailable")
    );
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn requested_certificate_absence_is_quiet_only_when_detail_has_converged() {
    use crate::app::terminal_coverage::{self, COVERED, NOT_ATTACHED};

    for incomplete in [false, true] {
        let mut request = api::AnalyzeRequest::default();
        request.limits.root_terminal_coverage = true;
        if incomplete {
            // Entry work cannot prepare a certificate under this ledger. Keep
            // the producer's actual frontier and absence, without editing it.
            request.limits.max_work = 1;
        }
        let report = evm_abstract_server::analyze::analyze(request).unwrap();
        report.validate().unwrap();
        assert!(report.metadata.limits.root_terminal_coverage);
        assert!(report.root_terminal_coverage.is_none());
        assert_eq!(report.ssa.complete, !incomplete);
        let expected_detail = if incomplete {
            assert_eq!(report.status, api::AnalysisStatus::Incomplete);
            assert!(!report.frontiers.is_empty());
            assert_eq!(terminal_coverage::status(&report), Some(NOT_ATTACHED));
            "Global model incomplete"
        } else {
            assert_eq!(report.status, api::AnalysisStatus::Converged);
            assert!(report.frontiers.is_empty());
            assert_eq!(terminal_coverage::status(&report), None);
            "Global model converged"
        };
        assert_eq!(terminal_coverage::detail_status(&report), expected_detail);

        let mut workspace = workspace();
        workspace.receive(Ok(api::AnalyzeReply {
            result: Ok(report.clone()),
        }));
        let context = Context::default();
        crate::palette::configure(&context);
        let mut time = 0.0;
        frame(&context, &mut workspace, &mut time, 1440.0, vec![]);
        let output = frame(&context, &mut workspace, &mut time, 1440.0, vec![]);
        let text = painted(&output);
        assert!(
            text.iter()
                .any(|(_, item)| item.galley.text() == expected_detail)
        );
        assert_eq!(
            text.iter()
                .any(|(_, item)| item.galley.text() == NOT_ATTACHED),
            incomplete
        );
        assert!(!text.iter().any(|(_, item)| item.galley.text() == COVERED));
        let live = workspace.accessible_status();
        assert_eq!(live.contains(NOT_ATTACHED), incomplete);
        assert!(!live.contains(COVERED));
        assert_eq!(workspace.report.as_ref().unwrap(), &report);
    }
}
