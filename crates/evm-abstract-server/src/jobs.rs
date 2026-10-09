//! A single owner schedules bounded, single-threaded analysis workers.
//!
//! HTTP handlers exchange typed commands with the coordinator. They never run
//! analysis, own mutable task state, or hold an execution lock while waiting.

mod actor;

use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode, ErrorDetails, JobId, JobSnapshot,
    WorkerErrorKind, WorkerFailure,
};
use std::{
    io,
    sync::{
        Arc,
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
}

#[derive(Clone)]
struct Native {
    providers: Arc<crate::rpc_providers::Registry>,
}
impl Executor for Native {
    fn execute(
        &self,
        request: AnalyzeRequest,
        control: &Control,
    ) -> Result<AnalysisReport, ApiError> {
        crate::analyze::analyze_with_control_and_providers(request, control, &self.providers)
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
        let providers = Arc::new(providers);
        Self::start(
            config,
            Native {
                providers: Arc::clone(&providers),
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
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Submit {
            request: Box::new(request),
            reply,
        })?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Read a small status snapshot without copying the report.
    pub fn status(&self, id: JobId) -> Result<JobSnapshot, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Status { id, reply })?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Request cooperative cancellation; terminal acknowledgement follows cleanup.
    pub fn cancel(&self, id: JobId) -> Result<JobSnapshot, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Cancel { id, reply })?;
        result.recv().map_err(|_| channel_error())?
    }

    /// Borrow an immutable completed report through a shared ownership handle.
    pub fn result(&self, id: JobId) -> Result<Arc<AnalysisReport>, ApiError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(actor::Command::Result { id, reply })?;
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
