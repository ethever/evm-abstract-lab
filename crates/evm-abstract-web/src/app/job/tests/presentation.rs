use super::*;
use crate::app::job::presentation::{DetailPresentation, ModelPresentation};
use evm_abstract_protocol as api;

fn minimum_report() -> api::AnalysisReport {
    let mut report = evm_abstract_server::analyze::analyze(AnalyzeRequest::default()).unwrap();
    // Omit optional rich views from a real producer model, as in the existing
    // staged-publication tests. No state, observation or currentness is minted.
    report.global = None;
    report.native = None;
    report.selected_view = api::AnalysisNamespace::Baseline;
    report.rich_export = api::RichExportOutcome::NotRequested;
    report.hybrid.as_mut().unwrap().local_prefixes.clear();
    report.validate().unwrap();
    report
}

#[test]
fn model_remains_ready_during_detail_cancellation_refusal_and_transport_recovery() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    let report = minimum_report();
    let view = task.presentation(Some(&report), Some(JobId(17)), false);
    assert_eq!(view.model, ModelPresentation::Current { ready: true });
    assert_eq!(view.detail, DetailPresentation::Running);
    assert_eq!(view.label(), "Model ready · detail running");
    assert!(view.cancellable && view.busy);
    task.receive_status(1, JobOperation::Poll, Err(disconnected()));
    let view = task.presentation(Some(&report), Some(JobId(17)), false);
    assert!(view.error.is_some());
    assert!(view.cancellable && view.busy);
    assert_eq!(view.model, ModelPresentation::Current { ready: true });
    assert!(matches!(
        task.apply_intent(crate::app::job::presentation::TaskIntent::Cancel),
        Some(Command::Cancel { id: JobId(17), .. })
    ));
    assert_eq!(
        task.presentation(Some(&report), Some(JobId(17)), false)
            .label(),
        "Model ready · cancelling detail"
    );
    status(&mut task, JobOperation::Cancel, JobState::Cancelled);
    let view = task.presentation(Some(&report), Some(JobId(17)), false);
    assert_eq!(view.model, ModelPresentation::Current { ready: true });
    assert_eq!(view.detail, DetailPresentation::Cancelled);
    assert!(!view.busy && !view.cancellable);

    let mut refused = report.clone();
    refused.rich_export = api::RichExportOutcome::Refused(api::ApiError {
        code: api::ApiErrorCode::Export,
        message: "optional presentation refused".into(),
        details: api::ErrorDetails::Worker(api::WorkerFailure {
            kind: api::WorkerErrorKind::Protocol,
            os_code: None,
        }),
    });
    refused.validate().unwrap();
    let view = task.presentation(Some(&refused), Some(JobId(17)), false);
    assert_eq!(view.model, ModelPresentation::Current { ready: true });
    assert_eq!(view.detail, DetailPresentation::Refused);
}

#[test]
fn new_job_never_labels_a_previous_ready_report_as_its_own_ready_model() {
    let report = minimum_report();
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    task.start(AnalyzeRequest::default());
    let view = task.presentation(Some(&report), Some(JobId(17)), true);
    assert_eq!(view.model, ModelPresentation::Previous { ready: true });
    assert_eq!(view.job_id, None);
    assert_eq!(view.report_job_id, Some(JobId(17)));
    assert_eq!(view.detail, DetailPresentation::Unavailable);
    assert_eq!(view.label(), "Submitting");
    task.receive_status(
        2,
        JobOperation::Submit,
        Ok(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(u64::MAX),
                publication: None,
                state: JobState::Running,
                progress: AnalysisProgress::default(),
                timings: api::AnalysisTimings::default(),
            }),
        }),
    );
    // Exact identity detects the prior result even without the shell hint.
    let view = task.presentation(Some(&report), Some(JobId(17)), false);
    assert_eq!(view.model, ModelPresentation::Previous { ready: true });
    assert_eq!(view.job_id.unwrap().to_string(), "18446744073709551615");
    assert_eq!(view.report_job_id, Some(JobId(17)));
    assert_eq!(view.phase, "Running");
    assert_eq!(view.label(), "Running");
}

#[test]
fn standalone_replacement_does_not_borrow_retired_job_or_unexported_native_readiness() {
    let report = minimum_report();
    let mut workspace = Workspace::default();
    workspace.initial_command();
    status(&mut workspace.task, JobOperation::Submit, JobState::Running);
    workspace.receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    let view = workspace.task_presentation();
    assert_eq!(view.model, ModelPresentation::Current { ready: true });
    assert_eq!(view.job_id, None);
    assert_eq!(view.report_job_id, None);
    assert_eq!(view.detail, DetailPresentation::Unavailable);
    assert_eq!(view.label(), "Model ready · detail unavailable");
    assert!(!view.busy && !view.cancellable);
}
