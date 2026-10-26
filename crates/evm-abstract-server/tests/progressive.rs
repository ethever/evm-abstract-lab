//! Real HTTP and Native execution keep model closure separate from task lifetime.

use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    AnalysisInput, AnalysisNamespace, AnalyzeRequest, ApiError, BytecodeInput, DecimalU64, JobId,
    JobReply, JobResult, JobResultReply, JobSnapshot, JobState, PublicationVersion,
};
use evm_abstract_server::{
    http::{router, serve_with_shutdown},
    jobs::{Config, Executor, Pool, Publisher},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Release(Arc<AtomicBool>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct FinalGate {
    projected: mpsc::SyncSender<()>,
    release: Arc<AtomicBool>,
    postprocess_work: usize,
}
impl Executor for FinalGate {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        evm_abstract_server::analyze::analyze_with_control(request, control)
    }

    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        publisher: &Publisher,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        let report = evm_abstract_server::analyze::analyze_with_publisher_and_limit(
            request,
            control,
            &Default::default(),
            publisher,
            self.postprocess_work,
        )?;
        self.projected.send(()).unwrap();
        // Fence final worker cleanup so the test can retain the genuine early
        // report and compare exact object/catalog identity after final install.
        while !self.release.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(1));
        }
        Ok(report)
    }
}

fn config() -> Config {
    Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 4,
    }
}

async fn status(client: &reqwest::Client, base: &str, id: JobId) -> JobSnapshot {
    client
        .get(format!("{base}/api/tasks/{id}"))
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap()
}

async fn result(client: &reqwest::Client, base: &str, id: JobId) -> JobResult {
    client
        .get(format!("{base}/api/tasks/{id}/result"))
        .send()
        .await
        .unwrap()
        .json::<JobResultReply>()
        .await
        .unwrap()
        .result
        .unwrap()
}

async fn terminal(client: &reqwest::Client, base: &str, id: JobId) -> JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = status(client, base, id).await;
        if matches!(
            snapshot.state,
            JobState::Completed | JobState::Cancelled | JobState::Failed(_)
        ) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not finish: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn server(
    pool: &Pool,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = router(std::env::temp_dir(), pool.clone());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        serve_with_shutdown(listener, app, async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    (base, stop, task)
}

#[tokio::test]
async fn high_budget_http_global_is_readable_while_native_runs_and_after_cancellation() {
    let pool = Pool::new(config()).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let mut request = AnalyzeRequest::default();
    // Use captured runtime bytes as a transport stress control. This is a
    // bytecode-input test, independent of full fixed-World corpus benchmarks.
    // Original unknown caller/value/calldata stay unchanged.
    let captured: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/known-recursion/captured/weth-26180958-observed-world.json"
    ))
    .unwrap();
    request.input = AnalysisInput::Bytecode(BytecodeInput {
        bytecode: captured["accounts"][0]["code"].as_str().unwrap().to_owned(),
        address: Some(
            captured["accounts"][0]["address"]
                .as_str()
                .unwrap()
                .to_owned(),
        ),
    });
    request.limits.max_work = 4_000_000_000;
    // Exercise continuing Native execution rather than the separately funded
    // optional terminal-envelope preparation. This does not restrict inputs.
    request.limits.root_terminal_coverage = false;
    let started = Instant::now();
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = status(&client, &base, job.id).await;
        assert!(matches!(snapshot.state, JobState::Running), "{snapshot:?}");
        if snapshot.publication.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closed global publication did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let early = result(&client, &base, job.id).await;
    assert_eq!(early.id, job.id);
    assert!(early.publication.model_closed);
    assert_eq!(early.publication.version, PublicationVersion(DecimalU64(1)));
    let minimum = early.report.minimum.as_ref().unwrap();
    minimum.validate().unwrap();
    assert!(minimum.current);
    assert!(!minimum.summaries.is_empty());
    assert_eq!(
        early.publication.minimum_version,
        Some(PublicationVersion(DecimalU64(1)))
    );
    assert!(early.publication.global_version.is_none());
    assert!(early.report.global.is_none());
    assert!(early.report.native.is_none());
    assert_eq!(early.report.selected_view, AnalysisNamespace::Baseline);
    assert!(
        early
            .report
            .hybrid
            .as_ref()
            .unwrap()
            .coverage
            .baseline_current
    );
    let published_at = started.elapsed();
    // A later cumulative execution receipt proves Native actually continued
    // after this report escaped through the socket, before requesting cancel.
    loop {
        let running = status(&client, &base, job.id).await;
        assert_eq!(
            running.state,
            JobState::Running,
            "early work={}, latest={running:?}, final={:?}",
            early.report.work,
            pool.result(job.id).ok().map(|report| report
                .hybrid
                .as_ref()
                .map(|h| &h.coverage)
                .cloned())
        );
        if running.progress.work > early.report.work {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no Native work after baseline publication"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let cancelling = client
        .delete(format!("{base}/api/tasks/{}", job.id))
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    assert!(matches!(
        cancelling.state,
        JobState::Cancelling | JobState::Cancelled
    ));
    let cancelled = terminal(&client, &base, job.id).await;
    assert_eq!(cancelled.state, JobState::Cancelled);
    let retained = result(&client, &base, job.id).await;
    assert_eq!(retained.publication, early.publication);
    assert_eq!(
        retained.report, early.report,
        "cancellation cannot edit published evidence"
    );
    eprintln!(
        "first closed HTTP model: {published_at:?}; cancelled: {:?}; work={}",
        started.elapsed(),
        cancelled.progress.work
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn final_native_publication_keeps_exact_minimum_identity_and_frozen_previous_handle() {
    let (projected, ready) = mpsc::sync_channel(1);
    let release = Arc::new(AtomicBool::new(false));
    let released = Release(Arc::clone(&release));
    let pool = Pool::with_executor(
        config(),
        FinalGate {
            projected,
            release,
            postprocess_work: evm_abstract_server::analyze::DEFAULT_POSTPROCESS_WORK,
        },
    )
    .unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let submitted = client
        .post(format!("{base}/api/tasks"))
        .json(&AnalyzeRequest::default())
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = pool.published_result(submitted.id).unwrap();
    assert_eq!(early.publication.version, PublicationVersion(DecimalU64(1)));
    assert!(early.report.native.is_none());
    drop(released);
    assert_eq!(
        terminal(&client, &base, submitted.id).await.state,
        JobState::Completed
    );
    let final_report = result(&client, &base, submitted.id).await;
    assert_eq!(
        final_report.publication.version,
        PublicationVersion(DecimalU64(2))
    );
    assert_eq!(
        final_report.publication.minimum_version,
        Some(PublicationVersion(DecimalU64(1)))
    );
    assert_eq!(
        final_report.publication.global_version,
        Some(PublicationVersion(DecimalU64(2)))
    );
    assert_eq!(
        final_report.publication.native_version,
        Some(PublicationVersion(DecimalU64(2)))
    );
    assert!(final_report.publication.model_closed);
    let handle = pool.published_result(submitted.id).unwrap();
    assert!(Arc::ptr_eq(
        early.report.minimum.as_ref().unwrap(),
        handle.report.minimum.as_ref().unwrap()
    ));
    assert!(handle.report.programs.len() >= early.report.programs.len());
    for (before, after) in early.report.programs.iter().zip(&handle.report.programs) {
        assert!(
            Arc::ptr_eq(before, after),
            "immutable source rows must keep instruction identities"
        );
        assert_eq!(before.id, after.id);
    }
    assert!(early.report.global.is_none());
    assert!(
        early.report.native.is_none(),
        "retained publication is immutable after final install"
    );
    let next = pool.published_result(submitted.id).unwrap();
    assert!(Arc::ptr_eq(&handle.report, &next.report));
    assert_eq!(handle.publication, next.publication);
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct Replacement;
impl Executor for Replacement {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        evm_abstract_server::analyze::analyze_with_control(request, control)
    }
    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        publisher: &Publisher,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        assert!(publisher.publish(self.execute(request.clone(), control)?));
        // A new original source/snapshot cannot inherit a previous graph version.
        let mut replacement = request;
        replacement.input = AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "600900".into(),
            address: None,
        });
        self.execute(replacement, control)
    }
}

#[tokio::test]
async fn generic_executor_replacement_gets_a_new_global_version() {
    let pool = Pool::with_executor(config(), Replacement).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&AnalyzeRequest::default())
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let replaced = result(&client, &base, job.id).await;
    assert_eq!(
        replaced.publication.version,
        PublicationVersion(DecimalU64(2))
    );
    assert_eq!(
        replaced.publication.global_version,
        Some(PublicationVersion(DecimalU64(2)))
    );
    assert_eq!(replaced.report.global.as_ref().unwrap().byte_len, 3);
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct RetainedFullGraphs;
impl Executor for RetainedFullGraphs {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        evm_abstract_server::analyze::analyze_with_control(request, control)
    }
    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        publisher: &Publisher,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        let report = self.execute(request, control)?;
        // Only envelope bookkeeping differs; these are the actual same immutable
        // rich Global/Native objects and source catalog, not equivalent rebuilds.
        let final_report = report.clone();
        assert!(publisher.publish(report));
        Ok(final_report)
    }
}

#[tokio::test]
async fn exact_rich_graph_reuse_preserves_both_namespace_versions() {
    let pool = Pool::with_executor(config(), RetainedFullGraphs).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&AnalyzeRequest::default())
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let retained = result(&client, &base, job.id).await;
    assert_eq!(
        retained.publication.version,
        PublicationVersion(DecimalU64(2))
    );
    assert_eq!(
        retained.publication.global_version,
        Some(PublicationVersion(DecimalU64(1)))
    );
    assert_eq!(
        retained.publication.native_version,
        Some(PublicationVersion(DecimalU64(1)))
    );
    assert_eq!(
        retained.publication.minimum_version,
        Some(PublicationVersion(DecimalU64(1)))
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct PureNative;
impl Executor for PureNative {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        let mut report = evm_abstract_server::analyze::analyze_with_control(request, control)?;
        report.global = None;
        report.minimum = None;
        report.hybrid = None;
        report.selected_view = AnalysisNamespace::Native;
        report.status = report.native.as_ref().unwrap().status;
        Ok(report)
    }
}

#[tokio::test]
async fn pure_native_result_has_no_global_graph_version() {
    let pool = Pool::with_executor(config(), PureNative).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let request = AnalyzeRequest {
        input: AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "00".into(),
            address: Some("0x0000000000000000000000000000000000000101".into()),
        }),
        ..Default::default()
    };
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let result = result(&client, &base, job.id).await;
    assert!(result.publication.global_version.is_none());
    assert!(result.publication.native_version.is_some());
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct RejectedFinalExport;
impl Executor for RejectedFinalExport {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        evm_abstract_server::analyze::analyze_with_control(request, control)
    }

    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        publisher: &Publisher,
    ) -> Result<evm_abstract_protocol::AnalysisReport, ApiError> {
        let report = self.execute(request, control)?;
        let metadata = &report.global.as_ref().unwrap().metadata;
        // Inject a typed final projector rejection at the executor boundary.
        // Real persistent-payer refusal is checked by CLI/projector budget tests;
        // this oracle checks actor and HTTP retention independently of them.
        let error = ApiError {
            code: evm_abstract_protocol::ApiErrorCode::Export,
            message: "final projection exceeded its node allowance".into(),
            details: evm_abstract_protocol::ErrorDetails::Export(Box::new(
                evm_abstract_protocol::ExportFailure {
                    kind: evm_abstract_protocol::ExportFailureKind::Nodes,
                    required: metadata.export_limits.max_nodes.saturating_add(1) as u64,
                    limits: metadata.export_limits,
                    usage: metadata.export_usage,
                    written: None,
                },
            )),
        };
        assert!(publisher.publish(report));
        Err(error)
    }
}

#[tokio::test]
async fn final_export_error_remains_failed_while_the_closed_publication_is_readable() {
    let pool = Pool::with_executor(config(), RejectedFinalExport).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&AnalyzeRequest::default())
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    let failed = terminal(&client, &base, job.id).await;
    let JobState::Failed(error) = failed.state else {
        panic!("missing final export failure")
    };
    assert_eq!(error.code, evm_abstract_protocol::ApiErrorCode::Export);
    let retained = result(&client, &base, job.id).await;
    assert_eq!(
        retained.publication.version,
        PublicationVersion(DecimalU64(1))
    );
    assert!(retained.publication.model_closed);
    assert_eq!(failed.publication.as_ref(), Some(&retained.publication));
    assert_eq!(
        status(&client, &base, job.id).await.state,
        JobState::Failed(error)
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn real_optional_rich_work_refusal_keeps_the_paid_minimum_over_http() {
    let request = AnalyzeRequest::default();
    let client = reqwest::Client::new();
    let (projected, ready) = mpsc::sync_channel(1);
    let release = Arc::new(AtomicBool::new(false));
    let released = Release(Arc::clone(&release));
    let calibration = Pool::with_executor(
        config(),
        FinalGate {
            projected,
            release,
            postprocess_work: 1_000_000,
        },
    )
    .unwrap();
    let (base, stop, task) = server(&calibration).await;
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let minimum_work = calibration
        .published_result(job.id)
        .unwrap()
        .report
        .postprocessing_work as usize;
    drop(released);
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let complete = result(&client, &base, job.id).await;
    assert!(matches!(
        complete.report.rich_export,
        evm_abstract_protocol::RichExportOutcome::Complete
    ));
    assert_eq!(
        complete
            .report
            .minimum
            .as_ref()
            .unwrap()
            .postprocessing_max_work,
        1_000_000
    );
    assert_eq!(
        complete
            .report
            .global
            .as_ref()
            .unwrap()
            .metadata
            .postprocessing_max_work,
        1_000_000
    );
    let full_work = complete.report.postprocessing_work as usize;
    assert!(full_work > minimum_work + 1);
    let limit = minimum_work + (full_work - minimum_work) / 2;
    stop.send(()).unwrap();
    task.await.unwrap();
    calibration.shutdown().unwrap();

    // Same semantic inputs and execution limits. Only the independently declared
    // application presentation allowance is lower, between minimum and full cost.
    let (projected, ready) = mpsc::sync_channel(1);
    let release = Arc::new(AtomicBool::new(false));
    let released = Release(Arc::clone(&release));
    let pool = Pool::with_executor(
        config(),
        FinalGate {
            projected,
            release,
            postprocess_work: limit,
        },
    )
    .unwrap();
    let (base, stop, task) = server(&pool).await;
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = result(&client, &base, job.id).await;
    let early_handle = pool.published_result(job.id).unwrap();
    assert_eq!(early.publication, early_handle.publication);
    assert!(early.publication.model_closed);
    assert!(early.report.global.is_none() && early.report.native.is_none());
    drop(released);
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let final_report = result(&client, &base, job.id).await;
    final_report.report.validate().unwrap();
    assert!(matches!(
        final_report.report.rich_export,
        evm_abstract_protocol::RichExportOutcome::Refused(_)
    ));
    assert!(final_report.publication.model_closed);
    assert_eq!(
        final_report.publication.minimum_version,
        early.publication.minimum_version
    );
    assert_eq!(final_report.report.minimum, early.report.minimum);
    assert!(final_report.report.postprocessing_work <= limit as u64);
    let final_handle = pool.published_result(job.id).unwrap();
    assert!(Arc::ptr_eq(
        early_handle.report.minimum.as_ref().unwrap(),
        final_handle.report.minimum.as_ref().unwrap()
    ));
    assert_eq!(&request, &AnalyzeRequest::default());
    eprintln!(
        "minimum work={minimum_work}; full work={full_work}; shared limit={limit}; spent={}",
        final_report.report.postprocessing_work
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();

    // Exercise the same declared policy through the production Native pool
    // constructor, independently of the cleanup fence used for Arc inspection.
    let pool = Pool::with_postprocess_work(config(), Default::default(), limit).unwrap();
    let (base, stop, task) = server(&pool).await;
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        terminal(&client, &base, job.id).await.state,
        JobState::Completed
    );
    let refused = result(&client, &base, job.id).await;
    assert!(matches!(
        refused.report.rich_export,
        evm_abstract_protocol::RichExportOutcome::Refused(_)
    ));
    assert_eq!(
        refused
            .report
            .minimum
            .as_ref()
            .unwrap()
            .postprocessing_max_work,
        limit as u64
    );
    assert!(refused.report.postprocessing_work <= limit as u64);
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn required_minimum_payment_failure_never_returns_a_rich_only_success() {
    let pool = Pool::with_postprocess_work(config(), Default::default(), 1).unwrap();
    let (base, stop, task) = server(&pool).await;
    let client = reqwest::Client::new();
    let job = client
        .post(format!("{base}/api/tasks"))
        .json(&AnalyzeRequest::default())
        .send()
        .await
        .unwrap()
        .json::<JobReply>()
        .await
        .unwrap()
        .result
        .unwrap();
    let failed = terminal(&client, &base, job.id).await;
    let JobState::Failed(error) = failed.state else {
        panic!("mandatory payment refusal was bypassed")
    };
    assert_eq!(error.code, evm_abstract_protocol::ApiErrorCode::Export);
    assert!(failed.publication.is_none());
    let response = client
        .get(format!("{base}/api/tasks/{}/result", job.id))
        .send()
        .await
        .unwrap();
    let reply = response.json::<JobResultReply>().await.unwrap();
    assert_eq!(reply.result.unwrap_err(), error);
    stop.send(()).unwrap();
    task.await.unwrap();
    pool.shutdown().unwrap();
}
