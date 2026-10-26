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

fn retained_rpc_precision_report() -> api::AnalysisReport {
    let mut report = evm_abstract_server::analyze::analyze(AnalyzeRequest::default()).unwrap();
    report.validate().unwrap();
    // The same validated consumer qualification profile as the shared model's
    // older-input fixture: retain actual producer minimum/Native receipts and
    // advance only the RPC restriction chain. No RPC execution is synthesized.
    report.global = None;
    let hybrid = report.hybrid.as_mut().unwrap();
    let precision = hybrid.native_coverage().clone();
    let revision = precision.detail_revision.unwrap();
    let current = api::DecimalU64(revision.0 + 1);
    let acquisition = api::AcquisitionReport {
        rounds: 1,
        fetched_accounts: vec![],
        failed_accounts: vec![],
        fetched_storage: vec![],
        failed_storage: vec![],
        failures: vec![],
        requests: 0,
        states_created: report.native.as_ref().unwrap().states.len(),
    };
    hybrid.rpc = Some(api::RpcHybridCoverage {
        snapshot: precision.snapshot.clone(),
        baseline_revision: hybrid.coverage.baseline_revision,
        current_revision: current,
        detail_revision: Some(revision),
        detail_at_current_revision: false,
        baseline_relation: api::RpcBaselineRelation::InitialFactRestriction,
        restrictions: vec![api::RpcFactRestriction {
            previous: hybrid.coverage.baseline_revision,
            current,
            added_observations: 1,
        }],
        precision: api::RpcPrecisionOutcome::Incomplete,
        acquisition: acquisition.clone(),
        work: report.work,
        precision_work: precision.work.preparation + precision.work.detail,
        max_precision_work: api::DecimalU64(precision.work.detail_allowance),
        transfers: report.transfers,
        failures: vec![],
        preparation_failure: None,
    });
    hybrid.precision = Some(precision);
    hybrid.coverage.chosen = api::HybridInterpretation::Process;
    report.acquisition = Some(acquisition);
    report.validate().unwrap();
    report
}

#[test]
fn retained_rpc_precision_drives_detail_status_without_requalifying_its_input() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    task.complete();
    let mut report = retained_rpc_precision_report();
    for (original, retained, expected) in [
        (
            api::HybridDetailState::Incomplete,
            api::HybridDetailState::Current,
            DetailPresentation::Ready,
        ),
        (
            api::HybridDetailState::Current,
            api::HybridDetailState::Incomplete,
            DetailPresentation::Incomplete,
        ),
    ] {
        let hybrid = report.hybrid.as_mut().unwrap();
        hybrid.coverage.detail = original;
        hybrid.coverage.detail_current = original == api::HybridDetailState::Current;
        let precision = hybrid.precision.as_mut().unwrap();
        precision.detail = retained;
        precision.detail_current = retained == api::HybridDetailState::Current;
        report.validate().unwrap();
        let before = report.clone();
        let view = task.presentation(Some(&report), Some(JobId(17)), false);
        assert_eq!(view.model, ModelPresentation::Current { ready: true });
        assert_eq!(view.detail, expected);
        assert_eq!(
            view.label(),
            if expected == DetailPresentation::Ready {
                "Model ready · detail ready"
            } else {
                "Model ready · detail incomplete"
            }
        );
        assert_eq!(
            report, before,
            "presentation cannot requalify retained evidence"
        );
        let hybrid = report.hybrid.as_ref().unwrap();
        let rpc = hybrid.rpc.as_ref().unwrap();
        let native = report.native.as_ref().unwrap();
        assert!(report.global.is_none() && report.minimum.as_ref().unwrap().current);
        assert!(hybrid.coverage.baseline_current);
        assert!(!rpc.detail_at_current_revision);
        assert_eq!(Some(native.metadata.revision), rpc.detail_revision);
        assert_ne!(native.metadata.revision, rpc.current_revision);
        assert!(matches!(
            hybrid.native_coverage().detail_binding,
            api::HybridNativeBinding::SymbolicOwnerNamespace(_)
        ));
        assert!(
            !native.ssa.blocks.is_empty(),
            "incomplete status cannot discard retained SSA"
        );
    }

    report.native = None;
    report.rich_export = api::RichExportOutcome::NotRequested;
    let hybrid = report.hybrid.as_mut().unwrap();
    hybrid.local_prefixes.clear();
    let precision = hybrid.precision.as_mut().unwrap();
    precision.detail = api::HybridDetailState::Current;
    precision.detail_current = true;
    report.validate().unwrap();
    assert_eq!(
        task.presentation(Some(&report), Some(JobId(17)), false)
            .detail,
        DetailPresentation::Unavailable,
        "precision metadata alone cannot advertise an exported Native graph"
    );
}
