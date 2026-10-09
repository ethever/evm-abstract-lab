//! Observe actual worker concurrency, queue admission and cleanup acknowledgements.

use evm_abstract::analysis::{
    control::Control,
    progress::{Event, Phase},
};
use evm_abstract_protocol::{
    AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode, ErrorDetails, JobId, JobSnapshot,
    JobState, ValidationErrorKind, ValidationFailure, WorkerErrorKind,
};
use evm_abstract_server::jobs::{Config, Executor, Pool};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

fn failure() -> ApiError {
    ApiError {
        code: ApiErrorCode::InvalidRequest,
        message: "test executor finished".into(),
        details: ErrorDetails::Validation(ValidationFailure {
            kind: ValidationErrorKind::Field,
            field: Some("fixture".into()),
            value: None,
        }),
    }
}

#[derive(Clone)]
struct Gate {
    entered: SyncSender<ThreadId>,
    release: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Release(Arc<AtomicBool>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Executor for Gate {
    fn execute(
        &self,
        _request: AnalyzeRequest,
        control: &Control,
    ) -> Result<AnalysisReport, ApiError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = Active(Arc::clone(&self.active));
        self.peak.fetch_max(active, Ordering::SeqCst);
        control.observer().phase(Phase::Analyzing);
        control.observer().emit(Event::Execution {
            states: 3,
            transfers: 7,
            work: 19,
        });
        self.entered.send(thread::current().id()).unwrap();
        // Simulate one non-interruptible native call. A cancellation request
        // cannot honestly acknowledge release before this call has returned.
        while !self.release.load(Ordering::Acquire) {
            thread::park_timeout(Duration::from_millis(1));
        }
        Err(failure())
    }
}

fn wait_terminal(pool: &Pool, id: JobId) -> JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = pool.status(id).unwrap();
        if matches!(
            status.state,
            JobState::Completed | JobState::Failed(_) | JobState::Cancelled
        ) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "task remained {:?}",
            status.state
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn gate() -> (Gate, mpsc::Receiver<ThreadId>, Release) {
    let (entered, receiver) = mpsc::sync_channel(16);
    let release = Arc::new(AtomicBool::new(false));
    let executor = Gate {
        entered,
        release: Arc::clone(&release),
        active: Arc::new(AtomicUsize::new(0)),
        peak: Arc::new(AtomicUsize::new(0)),
    };
    (executor, receiver, Release(release))
}

#[test]
fn bounded_workers_run_concurrently_and_queue_rejects_excess_work() {
    let (executor, entered, release) = gate();
    let pool = Pool::with_executor(
        Config {
            workers: 2,
            queue_capacity: 1,
            retained_jobs: 8,
        },
        executor.clone(),
    )
    .unwrap();
    let first = pool.submit(AnalyzeRequest::default()).unwrap();
    let second = pool.submit(AnalyzeRequest::default()).unwrap();
    let first_thread = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let second_thread = entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(first_thread, second_thread);
    assert_ne!(first_thread, thread::current().id());
    let queued = pool.submit(AnalyzeRequest::default()).unwrap();
    assert_eq!(queued.state, JobState::Queued);
    assert_eq!(
        pool.submit(AnalyzeRequest::default()).unwrap_err().code,
        ApiErrorCode::QueueFull
    );
    assert_eq!(executor.active.load(Ordering::SeqCst), 2);
    assert_eq!(executor.peak.load(Ordering::SeqCst), 2);
    drop(release);
    for id in [first.id, second.id, queued.id] {
        let status = wait_terminal(&pool, id);
        assert_eq!(status.state, JobState::Failed(failure()));
        assert_eq!(status.progress.transfers, 7);
        assert_eq!(status.progress.work, 19);
    }
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    pool.shutdown().unwrap();
}

#[test]
fn cancelling_waits_for_resource_release_and_queued_cancellation_never_runs() {
    let (executor, entered, release) = gate();
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 8,
        },
        executor.clone(),
    )
    .unwrap();
    let running = pool.submit(AnalyzeRequest::default()).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let queued = pool.submit(AnalyzeRequest::default()).unwrap();
    assert_eq!(pool.cancel(queued.id).unwrap().state, JobState::Cancelled);
    assert_eq!(pool.cancel(running.id).unwrap().state, JobState::Cancelling);
    assert_eq!(pool.status(running.id).unwrap().state, JobState::Cancelling);
    assert_eq!(
        pool.result(running.id).unwrap_err().code,
        ApiErrorCode::TaskNotReady
    );
    assert_eq!(executor.active.load(Ordering::SeqCst), 1);
    assert!(
        entered.try_recv().is_err(),
        "cancelled queued request entered the executor"
    );
    drop(release);
    assert_eq!(wait_terminal(&pool, running.id).state, JobState::Cancelled);
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        pool.result(running.id).unwrap_err().code,
        ApiErrorCode::Cancelled
    );
    let next = pool.submit(AnalyzeRequest::default()).unwrap();
    assert_eq!(
        wait_terminal(&pool, next.id).state,
        JobState::Failed(failure())
    );
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct PanicOnce(Arc<AtomicUsize>);
impl Executor for PanicOnce {
    fn execute(&self, _: AnalyzeRequest, _: &Control) -> Result<AnalysisReport, ApiError> {
        assert_ne!(
            self.0.fetch_add(1, Ordering::SeqCst),
            0,
            "deliberate test worker failure"
        );
        Err(failure())
    }
}

#[test]
fn panicking_worker_reports_typed_failure_and_is_replaced() {
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 2,
        },
        PanicOnce(Arc::new(AtomicUsize::new(0))),
    )
    .unwrap();
    let first = pool.submit(AnalyzeRequest::default()).unwrap();
    let JobState::Failed(error) = wait_terminal(&pool, first.id).state else {
        panic!("missing panic failure")
    };
    assert!(
        matches!(error.details, ErrorDetails::Worker(detail) if detail.kind == WorkerErrorKind::Panic)
    );
    let second = pool.submit(AnalyzeRequest::default()).unwrap();
    assert_eq!(
        wait_terminal(&pool, second.id).state,
        JobState::Failed(failure())
    );
    let third = pool.submit(AnalyzeRequest::default()).unwrap();
    wait_terminal(&pool, third.id);
    assert_eq!(
        pool.status(first.id).unwrap_err().code,
        ApiErrorCode::TaskNotFound
    );
    pool.shutdown().unwrap();
}

struct ExitWitness {
    worker: bool,
    entered: SyncSender<()>,
    dropping: SyncSender<()>,
    panic_now: Arc<AtomicBool>,
    release_drop: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
}
impl Clone for ExitWitness {
    fn clone(&self) -> Self {
        Self {
            worker: true,
            entered: self.entered.clone(),
            dropping: self.dropping.clone(),
            panic_now: Arc::clone(&self.panic_now),
            release_drop: Arc::clone(&self.release_drop),
            drops: Arc::clone(&self.drops),
        }
    }
}
impl Drop for ExitWitness {
    fn drop(&mut self) {
        if self.worker {
            let _ = self.dropping.send(());
            while !self.release_drop.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_millis(1));
            }
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}
impl Executor for ExitWitness {
    fn execute(&self, _: AnalyzeRequest, _: &Control) -> Result<AnalysisReport, ApiError> {
        self.entered.send(()).unwrap();
        while !self.panic_now.load(Ordering::Acquire) {
            thread::park_timeout(Duration::from_millis(1));
        }
        panic!("deliberate native worker panic before thread-owned resource cleanup")
    }
}

#[test]
fn cancellation_and_shutdown_acknowledge_joined_worker_cleanup() {
    let (entered, ready) = mpsc::sync_channel(1);
    let (dropping, teardown) = mpsc::sync_channel(4);
    let panic_now = Arc::new(AtomicBool::new(false));
    let release_drop = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let _panic_release = Release(Arc::clone(&panic_now));
    let _drop_release = Release(Arc::clone(&release_drop));
    let pool = Pool::with_executor(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 4,
        },
        ExitWitness {
            worker: false,
            entered,
            dropping,
            panic_now: Arc::clone(&panic_now),
            release_drop: Arc::clone(&release_drop),
            drops: Arc::clone(&drops),
        },
    )
    .unwrap();
    let job = pool.submit(AnalyzeRequest::default()).unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(pool.cancel(job.id).unwrap().state, JobState::Cancelling);
    panic_now.store(true, Ordering::Release);
    teardown.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        pool.status(job.id).unwrap().state,
        JobState::Cancelling,
        "an unwind guard is not proof that the worker's captured resources were dropped"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    release_drop.store(true, Ordering::Release);
    assert_eq!(wait_terminal(&pool, job.id).state, JobState::Cancelled);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    pool.shutdown().unwrap();
    assert_eq!(
        drops.load(Ordering::SeqCst),
        2,
        "shutdown must join the replacement worker too"
    );
}

#[test]
fn impossible_pool_capacity_is_a_typed_error_before_threads_are_started() {
    let result = Pool::new(Config {
        workers: usize::MAX,
        queue_capacity: 0,
        retained_jobs: 1,
    });
    assert!(matches!(
        result,
        Err(evm_abstract_server::jobs::PoolError::Capacity(_))
    ));
}
