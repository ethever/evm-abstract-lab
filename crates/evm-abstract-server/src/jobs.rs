//! A single owner schedules bounded, single-threaded analysis workers.
//!
//! HTTP handlers exchange typed commands with the coordinator. They never run
//! analysis, own mutable task state, or hold an execution lock while waiting.

mod actor;
mod task;

use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode, DecompileArtifact,
    DecompileJobSnapshot, DecompileRequest, ErrorDetails, JobId, JobPublication, JobSnapshot,
    PublicationVersion, WorkerErrorKind, WorkerFailure,
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
};
use thiserror::Error;

/// Process-local scheduling and retention bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// CPU analysis workers; defaults to half the host's available threads.
    pub workers: usize,
    /// Requests allowed to wait in addition to the active worker slots.
    pub queue_capacity: usize,
    /// Completed, failed and cancelled jobs retained for later inspection.
    pub retained_jobs: usize,
}

impl Default for Config {
    fn default() -> Self {
        let available = thread::available_parallelism().map_or(1, usize::from);
        let workers = (available / 2).max(1);
        Self {
            workers,
            queue_capacity: workers.saturating_mul(2),
            retained_jobs: 16,
        }
    }
}

/// Failure to construct a bounded pool, before accepting any jobs.
#[derive(Debug, Error)]
pub enum PoolError {
    /// A zero worker count or zero retained-history bound is invalid.
    #[error("workers and retained jobs must be positive")]
    Configuration,
    /// The requested pool bookkeeping capacity cannot be allocated.
    #[error("analysis pool capacity: {0}")]
    Capacity(std::collections::TryReserveError),
    /// An operating-system thread could not be created.
    #[error("starting analysis pool: {0}")]
    Spawn(io::Error),
}

/// A statically dispatched execution boundary, also usable by transport tests.
pub trait Executor: Clone + Send + 'static {
    /// Run the whole analysis on this worker thread, retaining typed failures.
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<AnalysisReport, ApiError>;

    /// Publish immutable results during execution when supported by this runner.
    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        _publisher: &Publisher,
    ) -> Result<AnalysisReport, ApiError> {
        self.execute(request, control)
    }

    /// Run an independent local decompilation on the same bounded worker slots.
    fn decompile(
        &self,
        request: DecompileRequest,
        control: &Control,
        progress: &crate::decompile::Observer,
    ) -> Result<DecompileArtifact, ApiError> {
        crate::decompile::run(request, control, progress)
    }
}

/// Worker-owned channel for at most a baseline and a final publication.
/// It sends handles to the coordinator, never waits for HTTP consumers.
pub struct Publisher {
    sender: SyncSender<actor::Command>,
    worker: usize,
    id: JobId,
    published: AtomicBool,
}

impl Publisher {
    /// Retain a report without changing the job's running/cancellation lifecycle.
    pub fn publish(&self, report: AnalysisReport) -> bool {
        self.publish_shared(Arc::new(report))
    }

    pub(crate) fn publish_shared(&self, report: Arc<AnalysisReport>) -> bool {
        // A worker can expose one early model, then its normal final result.
        // This keeps projection/channel retention bounded even for custom executors.
        if self.published.swap(true, Ordering::Relaxed) {
            return false;
        }
        let (installed, receipt) = mpsc::sync_channel(1);
        self.sender
            .send(actor::Command::Published {
                worker: self.worker,
                id: self.id,
                report,
                installed,
            })
            .is_ok()
            && receipt.recv().is_ok()
    }
}

/// One immutable result and its exact publication identity, without graph cloning.
#[derive(Debug)]
pub struct PublishedResult {
    /// Originating task.
    pub id: JobId,
    /// Version of this handle, independent of subsequent worker activity.
    pub publication: JobPublication,
    /// Immutable source-qualified report.
    pub report: Arc<AnalysisReport>,
}

/// Immutable decompiler result; its stages do not use native state identities.
#[derive(Debug)]
pub struct PublishedDecompileResult {
    /// Originating task in the common scheduler.
    pub id: JobId,
    /// Network publication version, independent of IR stages.
    pub publication: PublicationVersion,
    /// Independent transformation artifact.
    pub artifact: Arc<DecompileArtifact>,
}

#[derive(Clone)]
struct Native {
    providers: Arc<crate::rpc_providers::Registry>,
    postprocess_work: usize,
}
impl Executor for Native {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<AnalysisReport, ApiError> {
        crate::analyze::analyze_with_control_and_providers(request, control, &self.providers)
    }

    fn execute_with_publisher(
        &self,
        request: AnalyzeRequest,
        control: &Control,
        publisher: &Publisher,
    ) -> Result<AnalysisReport, ApiError> {
        crate::analyze::analyze_with_publisher_and_limit(
            request,
            control,
            &self.providers,
            publisher,
            self.postprocess_work,
        )
    }
}

struct Handle {
    sender: SyncSender<actor::Command>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.sender.send(actor::Command::Shutdown { reply: None });
    }
}

/// Cloneable typed client of the scheduler; clones do not create more workers.
#[derive(Clone)]
pub struct Pool {
    handle: Arc<Handle>,
    providers: Arc<crate::rpc_providers::Registry>,
}

impl Pool {
    /// Start a native analysis pool with explicit scheduling bounds.
    pub fn new(config: Config) -> Result<Self, PoolError> {
        Self::with_providers(config, crate::rpc_providers::Registry::default())
    }

    /// Start native workers sharing the backend-owned RPC configuration.
    pub fn with_providers(
        config: Config,
        providers: crate::rpc_providers::Registry,
    ) -> Result<Self, PoolError> {
        Self::with_postprocess_work(config, providers, crate::analyze::DEFAULT_POSTPROCESS_WORK)
    }

    /// Bound one persistent per-job allowance for minimum and rich projection.
    /// The request's EVM/RPC execution limits remain independent.
    pub fn with_postprocess_work(
        config: Config,
        providers: crate::rpc_providers::Registry,
        postprocess_work: usize,
    ) -> Result<Self, PoolError> {
        let providers = Arc::new(providers);
        Self::start(
            config,
            Native {
                providers: Arc::clone(&providers),
                postprocess_work,
            },
            providers,
        )
    }

    /// Start a pool around a concrete executor without dynamic callbacks.
    pub fn with_executor<E: Executor>(config: Config, executor: E) -> Result<Self, PoolError> {
        Self::start(
            config,
            executor,
            Arc::new(crate::rpc_providers::Registry::default()),
        )
    }

    fn start<E: Executor>(
        config: Config,
        executor: E,
        providers: Arc<crate::rpc_providers::Registry>,
    ) -> Result<Self, PoolError> {
        if config.workers == 0 || config.retained_jobs == 0 {
            return Err(PoolError::Configuration);
        }
        let (sender, receiver) = mpsc::sync_channel(128);
        let coordinator = actor::Coordinator::new(config, executor, sender.clone())?;
        thread::Builder::new()
            .name("analysis-coordinator".into())
            .spawn(move || coordinator.run(receiver))
            .map_err(PoolError::Spawn)?;
        Ok(Self {
            handle: Arc::new(Handle { sender }),
            providers,
        })
    }

    /// Configured provider names and URL values for the browser's selector.
    pub fn rpc_providers(&self) -> Vec<evm_abstract_protocol::RpcProvider> {
        self.providers.catalogue()
    }

    /// Admit a configured input or return a validation/queue-capacity failure.
    pub fn submit(&self, request: AnalyzeRequest) -> Result<JobSnapshot, ApiError> {
        self.providers.validate_request(&request)?;
        match self.submit_task(task::Request::Analysis(Box::new(request)))? {
            task::Snapshot::Analysis(snapshot) => Ok(snapshot),
            task::Snapshot::Decompile(_) => Err(task_type_error()),
        }
    }

    /// Admit local bytecode recovery without creating an execution world.
    pub fn submit_decompile(
        &self,
        request: DecompileRequest,
    ) -> Result<DecompileJobSnapshot, ApiError> {
        crate::decompile::validate(&request)?;
        match self.submit_task(task::Request::Decompile(Box::new(request)))? {
            task::Snapshot::Decompile(snapshot) => Ok(snapshot),
            task::Snapshot::Analysis(_) => Err(task_type_error()),
        }
    }

    fn submit_task(&self, request: task::Request) -> Result<task::Snapshot, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Submit {
            request: Box::new(request),
            reply,
        })?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Read a small status snapshot without copying the report.
    pub fn status(&self, id: JobId) -> Result<JobSnapshot, ApiError> {
        match self.task_status(id, task::Kind::Analysis, false)? {
            task::Snapshot::Analysis(snapshot) => Ok(snapshot),
            task::Snapshot::Decompile(_) => Err(task_type_error()),
        }
    }

    /// Read local recovery/structuring progress without cloning its artifact.
    pub fn decompile_status(&self, id: JobId) -> Result<DecompileJobSnapshot, ApiError> {
        match self.task_status(id, task::Kind::Decompile, false)? {
            task::Snapshot::Decompile(snapshot) => Ok(snapshot),
            task::Snapshot::Analysis(_) => Err(task_type_error()),
        }
    }

    fn task_status(
        &self,
        id: JobId,
        kind: task::Kind,
        cancel: bool,
    ) -> Result<task::Snapshot, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        let command = if cancel {
            actor::Command::Cancel { id, kind, reply }
        } else {
            actor::Command::Status { id, kind, reply }
        };
        self.send(command)?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Request cooperative cancellation; terminal acknowledgement follows cleanup.
    pub fn cancel(&self, id: JobId) -> Result<JobSnapshot, ApiError> {
        match self.task_status(id, task::Kind::Analysis, true)? {
            task::Snapshot::Analysis(snapshot) => Ok(snapshot),
            task::Snapshot::Decompile(_) => Err(task_type_error()),
        }
    }

    /// Request cancellation; terminal acknowledgment follows worker cleanup.
    pub fn cancel_decompile(&self, id: JobId) -> Result<DecompileJobSnapshot, ApiError> {
        match self.task_status(id, task::Kind::Decompile, true)? {
            task::Snapshot::Decompile(snapshot) => Ok(snapshot),
            task::Snapshot::Analysis(_) => Err(task_type_error()),
        }
    }

    /// Borrow the latest immutable report, including while optional work runs.
    pub fn result(&self, id: JobId) -> Result<Arc<AnalysisReport>, ApiError> {
        self.published_result(id).map(|result| result.report)
    }

    /// Read publication identity and report atomically; cancellation retains it.
    pub fn published_result(&self, id: JobId) -> Result<PublishedResult, ApiError> {
        match self.task_result(id, task::Kind::Analysis)? {
            task::Published::Analysis(result) => Ok(result),
            task::Published::Decompile(_) => Err(task_type_error()),
        }
    }

    /// Read the exact immutable decompiler publication.
    pub fn decompile_result(&self, id: JobId) -> Result<PublishedDecompileResult, ApiError> {
        match self.task_result(id, task::Kind::Decompile)? {
            task::Published::Decompile(result) => Ok(result),
            task::Published::Analysis(_) => Err(task_type_error()),
        }
    }

    fn task_result(&self, id: JobId, kind: task::Kind) -> Result<task::Published, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Result { id, kind, reply })?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Stop admission and wait until worker-owned work has been released.
    pub fn shutdown(&self) -> Result<(), ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Shutdown { reply: Some(reply) })?;
        result.recv().map_err(|_| channel_error())
    }

    fn send(&self, command: actor::Command) -> Result<(), ApiError> {
        self.handle
            .sender
            .send(command)
            .map_err(|_| channel_error())
    }
}

pub(crate) fn worker_error(kind: WorkerErrorKind, message: impl Into<String>) -> ApiError {
    ApiError {
        code: ApiErrorCode::WorkerFailed,
        message: message.into(),
        details: ErrorDetails::Worker(WorkerFailure {
            kind,
            os_code: None,
        }),
    }
}

pub(crate) fn worker_spawn_error(error: io::Error) -> ApiError {
    ApiError {
        code: ApiErrorCode::WorkerFailed,
        message: format!("starting analysis worker: {error}"),
        details: ErrorDetails::Worker(WorkerFailure {
            kind: WorkerErrorKind::Spawn,
            os_code: error.raw_os_error(),
        }),
    }
}

fn channel_error() -> ApiError {
    worker_error(
        WorkerErrorKind::Channel,
        "analysis coordinator is unavailable",
    )
}

fn task_type_error() -> ApiError {
    worker_error(
        WorkerErrorKind::Protocol,
        "task response has the wrong kind",
    )
}
