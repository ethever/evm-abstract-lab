//! Cross-product task lifecycle: decompilation shares capacity, not native state.

use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode, DecompileJobSnapshot, DecompileRequest,
    ErrorDetails, JobId, JobState, ValidationErrorKind, ValidationFailure,
    decompile_model::Completion,
};
use evm_abstract_server::jobs::{Config, Executor, Pool};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Gate {
    started: mpsc::SyncSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl Executor for Gate {
    fn execute(&self, _: AnalyzeRequest, _: &Control) -> Result<AnalysisReport, ApiError> {
        self.started.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Err(ApiError {
            code: ApiErrorCode::InvalidRequest,
            message: "deliberate gate completion".into(),
            details: ErrorDetails::Validation(ValidationFailure {
                kind: ValidationErrorKind::Field,
                field: Some("fixture".into()),
                value: None,
            }),
        })
    }
}

fn pool() -> (Pool, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
    let (started, waiting) = mpsc::sync_channel(1);
    let (release, receiver) = mpsc::sync_channel(1);
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 8,
        },
        Gate {
            started,
            release: Arc::new(Mutex::new(receiver)),
        },
    )
    .unwrap();
    (pool, waiting, release)
}

fn terminal(pool: &Pool, id: JobId) -> DecompileJobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = pool.decompile_status(id).unwrap();
        if matches!(
            snapshot.state,
            JobState::Completed | JobState::Cancelled | JobState::Failed(_)
        ) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "decompiler task failed to terminate"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn analysis_and_decompilation_share_queue_capacity_and_keep_result_types_separate() {
    let (pool, started, release) = pool();
    let analysis = pool.submit(AnalyzeRequest::default()).unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let local = pool.submit_decompile(DecompileRequest::default()).unwrap();
    assert_eq!(local.state, JobState::Queued);
    assert_eq!(
        pool.submit_decompile(DecompileRequest::default())
            .unwrap_err()
            .code,
        ApiErrorCode::QueueFull
    );

    // A request through the wrong API must not read or cancel a task of another kind.
    assert_eq!(
        pool.cancel(local.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    assert_eq!(
        pool.decompile_status(local.id).unwrap().state,
        JobState::Queued
    );
    assert_eq!(
        pool.cancel_decompile(analysis.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    assert_eq!(pool.status(analysis.id).unwrap().state, JobState::Running);
    release.send(()).unwrap();

    assert_eq!(terminal(&pool, local.id).state, JobState::Completed);
    let result = pool.decompile_result(local.id).unwrap();
    assert_eq!(result.artifact.completion, Completion::Complete);
    assert!(!result.artifact.stages.is_empty());
    assert!(!result.artifact.events.is_empty());
    assert_eq!(
        pool.published_result(local.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    assert_eq!(
        pool.decompile_result(analysis.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    pool.shutdown().unwrap();
}

#[test]
fn a_queued_decompiler_can_be_cancelled_without_executing_or_disturbing_native_work() {
    let (pool, started, release) = pool();
    let analysis = pool.submit(AnalyzeRequest::default()).unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let local = pool.submit_decompile(DecompileRequest::default()).unwrap();
    assert_eq!(
        pool.cancel_decompile(local.id).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        pool.decompile_result(local.id).unwrap_err().code,
        ApiErrorCode::Cancelled
    );
    assert_eq!(pool.status(analysis.id).unwrap().state, JobState::Running);
    release.send(()).unwrap();
    pool.shutdown().unwrap();
}

#[test]
fn unresolved_local_jump_publishes_an_inspectable_incomplete_artifact() {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 2,
    })
    .unwrap();
    let request = DecompileRequest {
        bytecode: "5f3556".into(),
        ..DecompileRequest::default()
    };
    let task = pool.submit_decompile(request).unwrap();
    assert_eq!(terminal(&pool, task.id).state, JobState::Completed);
    let result = pool.decompile_result(task.id).unwrap();
    assert_eq!(result.artifact.completion, Completion::Partial);
    assert!(!result.artifact.diagnostics.is_empty());
    assert!(!result.artifact.stages.is_empty());
    pool.shutdown().unwrap();
}

#[test]
fn malformed_hex_is_rejected_before_admission_and_valid_empty_code_is_allowed() {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 2,
    })
    .unwrap();
    for bytecode in ["0", "0xgg", "0x0"] {
        let request = DecompileRequest {
            bytecode: bytecode.into(),
            ..DecompileRequest::default()
        };
        assert_eq!(
            pool.submit_decompile(request).unwrap_err().code,
            ApiErrorCode::InvalidBytecode
        );
    }
    let task = pool
        .submit_decompile(DecompileRequest {
            bytecode: "0x".into(),
            ..DecompileRequest::default()
        })
        .unwrap();
    assert_eq!(terminal(&pool, task.id).state, JobState::Completed);
    assert_eq!(
        pool.decompile_result(task.id).unwrap().artifact.completion,
        Completion::Complete
    );
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct ControlledDecompile {
    started: mpsc::SyncSender<()>,
    saw_cancel: mpsc::SyncSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
    cleaned: Arc<AtomicBool>,
}

impl Executor for ControlledDecompile {
    fn execute(&self, _: AnalyzeRequest, _: &Control) -> Result<AnalysisReport, ApiError> {
        Err(ApiError {
            code: ApiErrorCode::InvalidRequest,
            message: "native follow-up fixture".into(),
            details: ErrorDetails::Validation(ValidationFailure {
                kind: ValidationErrorKind::Field,
                field: Some("fixture".into()),
                value: None,
            }),
        })
    }

    fn decompile(
        &self,
        _: DecompileRequest,
        control: &Control,
        progress: &evm_abstract_server::decompile::Observer,
    ) -> Result<evm_abstract_protocol::DecompileArtifact, ApiError> {
        let artifact = evm_decompile::decompile(
            &evm_decompile::Input::new(vec![0], evm_decompile::Fork::Osaka),
            &evm_decompile::Config::default(),
        );
        let observation = evm_abstract_protocol::decompile_model::Progress {
            phase: evm_abstract_protocol::decompile_model::Phase::Recovery,
            work: artifact.statistics.work,
            blocks: artifact.statistics.blocks,
            operations: artifact.statistics.operations,
            stage: None,
        };
        // Fill the bounded progress channel without blocking: the final installed
        // artifact must repair stage/phase even when the latest event is lost.
        for _ in 0..1_000 {
            progress.observe(observation.clone());
        }
        self.started.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !control.cancellation().is_cancelled() {
            assert!(
                Instant::now() < deadline,
                "test never requested cancellation"
            );
            thread::sleep(Duration::from_millis(1));
        }
        self.saw_cancel.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        self.cleaned.store(true, Ordering::Release);
        Ok(artifact)
    }
}

#[test]
fn running_cancel_waits_for_cleanup_and_preserves_an_exact_immutable_publication() {
    let (started, waiting) = mpsc::sync_channel(1);
    let (saw_cancel, cancelled) = mpsc::sync_channel(1);
    let (release, cleanup) = mpsc::sync_channel(1);
    let cleaned = Arc::new(AtomicBool::new(false));
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 1,
        },
        ControlledDecompile {
            started,
            saw_cancel,
            release: Arc::new(Mutex::new(cleanup)),
            cleaned: Arc::clone(&cleaned),
        },
    )
    .unwrap();
    let task = pool.submit_decompile(DecompileRequest::default()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        pool.cancel_decompile(task.id).unwrap().state,
        JobState::Cancelling
    );
    cancelled.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!cleaned.load(Ordering::Acquire));
    assert_eq!(
        pool.decompile_status(task.id).unwrap().state,
        JobState::Cancelling
    );
    assert_eq!(
        pool.decompile_result(task.id).unwrap_err().code,
        ApiErrorCode::TaskNotReady
    );
    release.send(()).unwrap();
    let done = terminal(&pool, task.id);
    assert_eq!(done.state, JobState::Cancelled);
    assert!(cleaned.load(Ordering::Acquire));
    let retained = pool.decompile_result(task.id).unwrap();
    assert_eq!(Some(retained.publication), done.publication);
    assert_eq!(done.progress.stage, retained.artifact.current_stage);
    assert_eq!(done.progress.work, retained.artifact.statistics.work);
    assert_eq!(
        done.progress.phase,
        evm_abstract_protocol::decompile_model::Phase::Complete
    );
    assert_eq!(pool.decompile_status(task.id).unwrap(), done);

    let native = pool.submit(AnalyzeRequest::default()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(pool.status(native.id).unwrap().state, JobState::Failed(_)) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        pool.decompile_result(task.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    // A reader's immutable handle survives bounded scheduler eviction.
    assert_eq!(retained.artifact.completion, Completion::Complete);
    assert!(!retained.artifact.stages.is_empty());
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct PanicDecompile {
    first: Arc<AtomicBool>,
    entered: mpsc::SyncSender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl Executor for PanicDecompile {
    fn execute(&self, _: AnalyzeRequest, _: &Control) -> Result<AnalysisReport, ApiError> {
        unreachable!("this fixture admits only decompiler requests")
    }
    fn decompile(
        &self,
        _: DecompileRequest,
        _: &Control,
        _: &evm_abstract_server::decompile::Observer,
    ) -> Result<evm_abstract_protocol::DecompileArtifact, ApiError> {
        if self.first.swap(false, Ordering::AcqRel) {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            panic!("deliberate decompiler worker panic");
        }
        Ok(evm_decompile::decompile(
            &evm_decompile::Input::new(vec![0], evm_decompile::Fork::Osaka),
            &evm_decompile::Config::default(),
        ))
    }
}

#[test]
fn panicking_decompiler_is_reported_and_a_replacement_serves_the_queued_request() {
    let (entered, waiting) = mpsc::sync_channel(1);
    let (release, receiver) = mpsc::sync_channel(1);
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 3,
        },
        PanicDecompile {
            first: Arc::new(AtomicBool::new(true)),
            entered,
            release: Arc::new(Mutex::new(receiver)),
        },
    )
    .unwrap();
    let first = pool.submit_decompile(DecompileRequest::default()).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let next = pool.submit_decompile(DecompileRequest::default()).unwrap();
    assert_eq!(next.state, JobState::Queued);
    release.send(()).unwrap();
    let JobState::Failed(error) = terminal(&pool, first.id).state else {
        panic!("panic must produce a typed worker failure");
    };
    assert_eq!(error.code, ApiErrorCode::WorkerFailed);
    let ErrorDetails::Worker(detail) = error.details else {
        panic!("worker failure details missing");
    };
    assert_eq!(detail.kind, evm_abstract_protocol::WorkerErrorKind::Panic);
    assert_eq!(terminal(&pool, next.id).state, JobState::Completed);
    assert_eq!(
        pool.decompile_result(next.id).unwrap().artifact.completion,
        Completion::Complete
    );
    pool.shutdown().unwrap();
}
