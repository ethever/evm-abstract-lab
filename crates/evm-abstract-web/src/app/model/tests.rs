use super::*;
use evm_abstract_protocol::{AnalysisInput, AnalyzeReply, AnalyzeRequest, BytecodeInput};

fn render(workspace: &mut Workspace, context: &egui::Context) -> Vec<String> {
    let mut text = Vec::new();
    for _ in 0..3 {
        let output = crate::tests::frame(context, workspace, vec![]);
        text = crate::tests::painted_text(&output)
            .into_iter()
            .map(|(text, _)| text)
            .collect();
    }
    text
}

#[test]
fn replacement_changes_cached_text_with_the_same_input_fingerprint_shape_and_ids() {
    let context = egui::Context::default();
    crate::palette::configure(&context);
    let mut workspace = Workspace {
        view: super::super::View::Ssa,
        ssa_display: crate::ssa_display::SsaDisplay::Full,
        ..Default::default()
    };
    let mut report = crate::tests::report();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((report.clone()).into()),
    }));
    let first = workspace.report_view.identity;
    let original = render(&mut workspace, &context);
    assert!(!original.iter().any(|text| text.contains("NEW_RECEIPT")));
    report.ssa.blocks[0].instructions[0].name = "NEW_RECEIPT".into();
    crate::tests::refresh_operation(&mut report.ssa.blocks[0].instructions[0]);
    report.validate().unwrap();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((report).into()),
    }));
    assert_ne!(workspace.report_view.identity, first);
    assert!(
        render(&mut workspace, &context)
            .iter()
            .any(|text| text.contains("NEW_RECEIPT"))
    );
}

#[test]
fn namespace_activation_never_carries_repeated_numeric_ids_or_ssa_focus_between_graphs() {
    let mut report = evm_abstract_server::analyze::analyze(AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "600160020100".into(),
            address: None,
        }),
        ..Default::default()
    })
    .unwrap();
    // This control selects the explicit rich graph namespaces. Canonical
    // minimum/Native switching has separate minimum-view regressions.
    report.minimum = None;
    report.validate().unwrap();
    let global = report.graph(AnalysisNamespace::Baseline).unwrap();
    let native = report.graph(AnalysisNamespace::Native).unwrap();
    assert_eq!(global.cfg[0].id, native.cfg[0].id);
    let global_first = (
        global.cfg[0].id,
        global.cfg[0].program.or(global.metadata.root_program),
    );
    let native_first = (
        native.cfg[0].id,
        native.cfg[0].program.or(native.metadata.root_program),
    );
    let context = egui::Context::default();
    crate::palette::configure(&context);
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    workspace.activate_view(Activation::Namespace(AnalysisNamespace::Native));
    render(&mut workspace, &context);
    workspace.graph.zoom_at(egui::vec2(70.0, 90.0), 0.7);
    let camera_zoom = workspace.graph.zoom;
    for (namespace, (state, program)) in [
        (AnalysisNamespace::Baseline, global_first),
        (AnalysisNamespace::Native, native_first),
        (AnalysisNamespace::Baseline, global_first),
    ] {
        workspace.selection.pc = Some(99);
        workspace.selection.value = Some(99);
        workspace.ssa_focus = workspace.selection;
        workspace.account = Some("old report account".into());
        workspace.activate_view(Activation::Namespace(namespace));
        assert_eq!(
            workspace.selection,
            Selection {
                state: Some(state),
                ..Default::default()
            }
        );
        assert_eq!(workspace.program, program);
        assert_eq!(workspace.account, None);
        assert_eq!(workspace.ssa_focus, Selection::default());
        render(&mut workspace, &context);
        assert_eq!(workspace.graph.zoom, camera_zoom);
    }
}

#[test]
fn unavailable_namespace_is_rejected_without_relabeling_the_visible_graph() {
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((crate::tests::report()).into()),
    }));
    let original = workspace.report_view.identity;
    let selection = workspace.selection;
    workspace.activate_view(Activation::Namespace(AnalysisNamespace::Baseline));
    assert_eq!(workspace.report_view.identity, original);
    assert_eq!(workspace.selection, selection);
}

fn publication(
    version: u64,
    global: Option<u64>,
    native: Option<u64>,
) -> evm_abstract_protocol::JobPublication {
    use evm_abstract_protocol::{DecimalU64, JobPublication, PublicationVersion};
    let sequence = |value| PublicationVersion(DecimalU64(value));
    JobPublication {
        version: sequence(version),
        minimum_version: None,
        global_version: global.map(sequence),
        native_version: native.map(sequence),
        model_closed: true,
    }
}

fn status(
    workspace: &mut Workspace,
    state: evm_abstract_protocol::JobState,
    publication: evm_abstract_protocol::JobPublication,
) {
    use evm_abstract_protocol::{AnalysisProgress, AnalysisTimings, JobId, JobReply, JobSnapshot};
    workspace.receive_message(super::super::Message::Status {
        generation: workspace.task.generation(),
        operation: super::super::JobOperation::Poll,
        result: Box::new(Ok(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(17),
                state,
                publication: Some(publication),
                progress: AnalysisProgress::default(),
                timings: AnalysisTimings::default(),
            }),
        })),
    });
}

fn fetch(workspace: &mut Workspace, now: f64) -> (u64, evm_abstract_protocol::JobId) {
    match workspace
        .task
        .next_command(now)
        .expect("a new immutable publication must be fetched")
    {
        super::super::Command::Result { generation, id } => (generation, id),
        _ => panic!("expected report download"),
    }
}

fn result(
    workspace: &mut Workspace,
    generation: u64,
    id: evm_abstract_protocol::JobId,
    publication: evm_abstract_protocol::JobPublication,
    report: AnalysisReport,
) {
    use evm_abstract_protocol::{JobResult, JobResultReply};
    workspace.receive_message(super::super::Message::Result {
        generation,
        id,
        result: Box::new(Ok(JobResultReply {
            result: Ok(JobResult {
                id,
                publication,
                report,
            }),
        })),
        transfer_ns: 1,
        parsing_ns: 1,
    });
}

fn global_without_detail(mut report: AnalysisReport) -> AnalysisReport {
    // Keep the actual producer's closed global graph and source directory.
    // This fixture removes optional observations to exercise staged UI arrival;
    // the real HTTP/browser case proves the producer's publication ordering.
    report.native = None;
    report.selected_view = AnalysisNamespace::Baseline;
    let hybrid = report.hybrid.as_mut().unwrap();
    hybrid.local_prefixes.clear();
    for coverage in std::iter::once(&mut hybrid.coverage).chain(hybrid.precision.iter_mut()) {
        coverage.chosen = evm_abstract_protocol::HybridInterpretation::Process;
        coverage.detail_revision = None;
        coverage.detail_config = None;
        coverage.detail_current = false;
        coverage.local_detail_sites = 0;
    }
    report.validate().unwrap();
    report
}

#[test]
fn native_arrival_retains_the_unchanged_global_selection_scene_camera_and_preferences() {
    let mut complete = evm_abstract_server::analyze::analyze(AnalyzeRequest::default()).unwrap();
    complete.minimum = None;
    let first = global_without_detail(complete.clone());
    let mut workspace = Workspace::default();
    workspace.initial_command();
    let v1 = publication(1, Some(1), None);
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Running,
        v1.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 0.0);
    result(&mut workspace, generation, id, v1, first);
    assert_eq!(workspace.task.phase, super::super::TaskPhase::Running);
    assert!(workspace.task.cancellable());
    let context = egui::Context::default();
    crate::palette::configure(&context);
    workspace.view = super::super::View::Graph;
    workspace.graph.set_mode(crate::widgets::GraphMode::Local);
    render(&mut workspace, &context);
    workspace.graph.zoom_at(egui::vec2(70.0, 90.0), 0.7);
    workspace.directory.open = false;
    workspace.ssa_display = crate::ssa_display::SsaDisplay::Full;
    workspace.inspector.expanded = true;
    let before = (
        workspace.report_view.identity,
        workspace.selection,
        workspace.program,
        workspace.graph.camera(),
    );
    let v2 = publication(2, Some(1), Some(2));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Completed,
        v2.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 1.0);
    result(&mut workspace, generation, id, v2, complete);
    render(&mut workspace, &context);
    assert_eq!(
        (
            workspace.report_view.identity,
            workspace.selection,
            workspace.program,
            workspace.graph.camera()
        ),
        before
    );
    assert!(!workspace.directory.open);
    assert!(workspace.inspector.expanded);
    assert_eq!(workspace.ssa_display, crate::ssa_display::SsaDisplay::Full);
    assert_eq!(workspace.task.phase, super::super::TaskPhase::Ready);
    workspace.activate_view(Activation::Namespace(AnalysisNamespace::Native));
    assert_eq!(workspace.report_view.namespace(), AnalysisNamespace::Native);
    assert_eq!(workspace.selection.pc, None);
    assert_eq!(workspace.selection.value, None);
    render(&mut workspace, &context);
    assert_eq!(workspace.graph.camera(), before.3);
}

#[test]
fn advertised_newer_publication_and_replacement_retire_in_flight_results() {
    let mut workspace = Workspace::default();
    workspace.initial_command();
    let v1 = publication(1, None, Some(1));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Running,
        v1.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 0.0);
    let v2 = publication(2, None, Some(2));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Running,
        v2.clone(),
    );
    result(
        &mut workspace,
        generation,
        id,
        v1,
        crate::tests::report().into(),
    );
    assert!(
        workspace.report.is_none(),
        "an older response cannot satisfy a newer advertised publication"
    );
    let (generation, id) = fetch(&mut workspace, 1.0);
    result(
        &mut workspace,
        generation,
        id,
        v2.clone(),
        crate::tests::report().into(),
    );
    let installed = workspace.report.clone();
    workspace.initial_command();
    assert!(workspace.previous_report);
    assert_eq!(workspace.report, installed);
    result(
        &mut workspace,
        generation,
        id,
        v2,
        crate::tests::empty_report().into(),
    );
    assert_eq!(workspace.report, installed);
    let v3 = publication(3, None, Some(3));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Running,
        v3.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 2.0);
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(crate::tests::report().into()),
    }));
    let replaced = workspace.report.clone();
    result(
        &mut workspace,
        generation,
        id,
        v3,
        crate::tests::empty_report().into(),
    );
    assert_eq!(workspace.report, replaced);
    assert_eq!(workspace.report_job_id, None);
    assert!(!workspace.task.needs_refresh());
    assert!(workspace.task.next_command(10.0).is_none());
}

#[test]
fn terminal_publication_retries_repaint_and_keeps_the_worker_failure_after_installation() {
    use evm_abstract_protocol::{ApiError, ApiErrorCode, ErrorDetails, JobState};
    for failed in [false, true] {
        let mut workspace = Workspace::default();
        workspace.initial_command();
        let failure = ApiError {
            code: ApiErrorCode::WorkerFailed,
            message: "final detail export failed".into(),
            details: ErrorDetails::Worker(evm_abstract_protocol::WorkerFailure {
                kind: evm_abstract_protocol::WorkerErrorKind::Protocol,
                os_code: None,
            }),
        };
        let state = if failed {
            JobState::Failed(failure)
        } else {
            JobState::Cancelled
        };
        let v1 = publication(1, None, Some(1));
        status(&mut workspace, state.clone(), v1.clone());
        assert!(!workspace.task.busy());
        assert!(workspace.task.needs_refresh());
        let context = egui::Context::default();
        crate::palette::configure(&context);
        if failed {
            assert!(workspace.task.next_command(0.0).is_none());
        } else {
            let (generation, id) = fetch(&mut workspace, 0.0);
            workspace.receive_message(super::super::Message::Result {
                generation,
                id,
                result: Box::new(Err(super::super::TransportError::Timeout { seconds: 60 })),
                transfer_ns: 1,
                parsing_ns: 0,
            });
        }
        let output = crate::tests::frame(&context, &mut workspace, vec![]);
        assert!(
            output
                .viewport_output
                .values()
                .any(|viewport| viewport.repaint_delay <= std::time::Duration::from_millis(250))
        );
        assert!(workspace.task.needs_refresh());
        let (generation, id) = fetch(&mut workspace, 2.0);
        result(
            &mut workspace,
            generation,
            id,
            v1,
            crate::tests::report().into(),
        );
        assert!(workspace.report.is_some());
        assert_eq!(workspace.task.snapshot.as_ref().unwrap().state, state);
        assert_eq!(workspace.task.error.is_some(), failed);
        if failed {
            assert!(
                workspace
                    .accessible_status()
                    .contains("final detail export failed")
            );
        }
        assert!(!workspace.task.needs_refresh());
    }
}

#[test]
fn a_reused_server_job_id_and_version_in_a_new_submission_still_replace_the_scene() {
    let mut workspace = Workspace::default();
    let context = egui::Context::default();
    crate::palette::configure(&context);
    workspace.view = super::super::View::Ssa;
    workspace.ssa_display = crate::ssa_display::SsaDisplay::Full;
    for name in ["FIRST_SERVER", "RESTARTED_SERVER"] {
        let mut report = crate::tests::report();
        report.ssa.blocks[0].instructions[0].name = name.into();
        crate::tests::refresh_operation(&mut report.ssa.blocks[0].instructions[0]);
        workspace.initial_command();
        let old = workspace.report_view.identity;
        let version = publication(1, None, Some(1));
        status(
            &mut workspace,
            evm_abstract_protocol::JobState::Running,
            version.clone(),
        );
        let (generation, id) = fetch(&mut workspace, 0.0);
        result(&mut workspace, generation, id, version, report.into());
        assert_ne!(workspace.report_view.identity, old);
        assert!(
            render(&mut workspace, &context)
                .iter()
                .any(|text| text.contains(name))
        );
    }
}

#[test]
fn invalid_staged_contents_keep_native_cancellable_and_recover_only_on_a_new_version() {
    for structural in [false, true] {
        let mut workspace = Workspace::default();
        workspace.receive(Ok(AnalyzeReply {
            result: Ok(crate::tests::report().into()),
        }));
        let old = workspace.report.clone();
        workspace.initial_command();
        let version = publication(1, None, Some(1));
        status(
            &mut workspace,
            evm_abstract_protocol::JobState::Running,
            version.clone(),
        );
        let (generation, id) = fetch(&mut workspace, 0.0);
        let mut invalid: AnalysisReport = crate::tests::report().into();
        if structural {
            std::sync::Arc::make_mut(invalid.native.as_mut().unwrap())
                .metadata
                .postprocessing_max_work = 0;
        } else {
            invalid.schema_version = 0;
        }
        result(&mut workspace, generation, id, version, invalid);
        assert_eq!(workspace.report, old);
        assert!(workspace.previous_report);
        assert!(workspace.task.cancellable());
        assert!(workspace.task.needs_refresh());
        assert!(workspace.task.next_command(0.0).is_none());
        assert!(matches!(
            workspace.task.next_command(1.1),
            Some(super::super::Command::Poll { .. })
        ));
        let version = publication(2, None, Some(2));
        status(
            &mut workspace,
            evm_abstract_protocol::JobState::Completed,
            version.clone(),
        );
        let (generation, id) = fetch(&mut workspace, 2.0);
        result(
            &mut workspace,
            generation,
            id,
            version,
            crate::tests::report().into(),
        );
        assert_eq!(workspace.task.phase, super::super::TaskPhase::Ready);
        assert!(!workspace.previous_report);
        assert!(workspace.task.error.is_none());
    }
}

#[test]
fn invalid_final_publication_stops_download_retries_without_erasing_the_previous_report() {
    for structural in [false, true] {
        let mut workspace = Workspace::default();
        workspace.receive(Ok(AnalyzeReply {
            result: Ok(crate::tests::report().into()),
        }));
        let old = workspace.report.clone();
        workspace.initial_command();
        let version = publication(1, None, Some(1));
        status(
            &mut workspace,
            evm_abstract_protocol::JobState::Completed,
            version.clone(),
        );
        let (generation, id) = fetch(&mut workspace, 0.0);
        let mut invalid: AnalysisReport = crate::tests::report().into();
        if structural {
            std::sync::Arc::make_mut(invalid.native.as_mut().unwrap())
                .metadata
                .postprocessing_max_work = 0;
        } else {
            invalid.schema_version = 0;
        }
        result(&mut workspace, generation, id, version, invalid);
        assert_eq!(workspace.report, old);
        assert_eq!(workspace.task.phase, super::super::TaskPhase::Failed);
        assert!(!workspace.task.needs_refresh());
        assert!(workspace.task.next_command(10.0).is_none());
    }
}

#[test]
fn catalog_growth_keeps_canonical_focus_and_binds_the_exact_installed_job() {
    use super::{navigation::CameraBookmark, presentation::PublicationIdentity};
    use std::sync::Arc;
    let mut workspace = Workspace::default();
    workspace.initial_command();
    let mut report: AnalysisReport = crate::tests::report().into();
    let first = publication(1, None, Some(1));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Running,
        first.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 0.0);
    result(&mut workspace, generation, id, first, report.clone());
    let view = workspace.report_view.identity();
    let camera = CameraBookmark::new(0.75, [17.0, -23.0]).unwrap();
    workspace
        .report_view
        .navigator_mut()
        .remember_camera(camera);
    let focus = workspace
        .report_view
        .navigator()
        .current()
        .cloned()
        .unwrap();
    assert_eq!(workspace.report_view.index().identity().job, Some(id));

    let mut alias = report.programs[0].as_ref().clone();
    alias.id = report.programs.len();
    alias.code_address = Some(format!("0x{}02", "00".repeat(19)));
    report.programs.push(Arc::new(alias));
    report.validate().unwrap();
    let richer = publication(2, None, Some(1));
    status(
        &mut workspace,
        evm_abstract_protocol::JobState::Completed,
        richer.clone(),
    );
    let (generation, id) = fetch(&mut workspace, 1.0);
    result(&mut workspace, generation, id, richer.clone(), report);
    assert_eq!(workspace.report_view.identity(), view);
    assert_eq!(workspace.report_view.navigator().current(), Some(&focus));
    let presentation = workspace
        .report_view
        .presentation(workspace.report.as_ref().unwrap());
    assert!(
        presentation.program(1).is_some(),
        "new catalog aliases are indexed even when graph version stays fixed"
    );
    assert_eq!(
        presentation.identity(),
        PublicationIdentity {
            report: view.generation(),
            job: Some(id),
            publication: Some(richer.version),
            minimum: None,
            global: None,
            native: richer.native_version,
        }
    );
}

#[test]
fn report_replacement_retires_canonical_history_and_old_numeric_source_handles() {
    use super::{
        navigation::{Intent, NavigationError},
        presentation::ContextTarget,
    };
    let mut workspace = Workspace::default();
    let report: AnalysisReport = crate::tests::report().into();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(report.clone()),
    }));
    let first = workspace
        .report_view
        .navigator()
        .current()
        .unwrap()
        .focus
        .clone();
    let next = workspace
        .report_view
        .presentation(&report)
        .focus_context(ContextTarget {
            view: first.view,
            context: navigation::ContextFocus::Graph { state: 2 },
        })
        .unwrap();
    workspace
        .report_view
        .navigate(&report, Intent::Open(next))
        .unwrap();
    assert!(workspace.report_view.navigator().can_back());
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    assert!(!workspace.report_view.navigator().can_back());
    assert!(!workspace.report_view.navigator().can_forward());
    assert_ne!(workspace.report_view.identity(), first.view);
    assert_eq!(
        workspace
            .report_view
            .navigate(workspace.report.as_ref().unwrap(), Intent::Open(first)),
        Err(NavigationError::StaleReport)
    );
}
