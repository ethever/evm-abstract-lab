//! Single-writer task lifecycle and explicit per-worker channels.

use super::{Config, Executor, PoolError, worker_error, worker_spawn_error};
use evm_abstract::analysis::{
    control::Control,
    progress::{Event, Observer, Phase},
};
use evm_abstract_protocol::{
    AnalysisPhase, AnalysisProgress, AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode,
    ErrorDetails, JobId, JobSnapshot, JobState, TaskErrorKind, TaskFailure, WorkerErrorKind,
};
use std::{
    collections::{BTreeMap, VecDeque, hash_map::RandomState},
    hash::BuildHasher,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

// One process-wide sequence prevents a replaced pool from answering stale IDs.
// A randomized process prefix also prevents practical reuse after server restart.
// This is task identity, not an authentication credential.
static NEXT_ID: OnceLock<AtomicU64> = OnceLock::new();
fn next_id() -> Result<JobId, ApiError> {
    NEXT_ID
        .get_or_init(|| {
            AtomicU64::new(
                (RandomState::new().hash_one(std::process::id()) & (u64::MAX >> 1)).max(1),
            )
        })
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(JobId)
        .map_err(|_| worker_error(WorkerErrorKind::Protocol, "task identity space exhausted"))
}

pub(super) enum Command {
    Submit {
        request: Box<AnalyzeRequest>,
        reply: SyncSender<Result<JobSnapshot, ApiError>>,
    },
    Status {
        id: JobId,
        reply: SyncSender<Result<JobSnapshot, ApiError>>,
    },
    Cancel {
        id: JobId,
        reply: SyncSender<Result<JobSnapshot, ApiError>>,
    },
    Result {
        id: JobId,
        reply: SyncSender<Result<Arc<AnalysisReport>, ApiError>>,
    },
    Finished {
        worker: usize,
        id: JobId,
        result: Box<Result<AnalysisReport, ApiError>>,
    },
    Stopped {
        worker: usize,
        error: Option<Box<ApiError>>,
    },
    Shutdown {
        reply: Option<SyncSender<()>>,
    },
}

struct Work {
    id: JobId,
    request: AnalyzeRequest,
    control: Control,
}

struct Worker {
    sender: Option<SyncSender<Work>>,
    active: Option<JobId>,
}

struct Entry {
    snapshot: JobSnapshot,
    control: Control,
    events: Receiver<Event>,
    request: Option<AnalyzeRequest>,
    report: Option<Arc<AnalysisReport>>,
}

pub(super) struct Coordinator<E> {
    config: Config,
    executor: E,
    sender: SyncSender<Command>,
    workers: Vec<Option<Worker>>,
    jobs: BTreeMap<JobId, Entry>,
    waiting: VecDeque<JobId>,
    retained: VecDeque<JobId>,
    stopping: bool,
    shutdown_replies: Vec<SyncSender<()>>,
}

impl<E: Executor> Coordinator<E> {
    pub(super) fn new(
        config: Config,
        executor: E,
        sender: SyncSender<Command>,
    ) -> Result<Self, PoolError> {
        let mut workers = Vec::new();
        workers
            .try_reserve_exact(config.workers)
            .map_err(PoolError::Capacity)?;
        for worker in 0..config.workers {
            workers.push(Some(
                start_worker(worker, executor.clone(), sender.clone()).map_err(PoolError::Spawn)?,
            ));
        }
        Ok(Self {
            config,
            executor,
            sender,
            workers,
            jobs: BTreeMap::new(),
            waiting: VecDeque::new(),
            retained: VecDeque::new(),
            stopping: false,
            shutdown_replies: Vec::new(),
        })
    }

    pub(super) fn run(mut self, receiver: Receiver<Command>) {
        loop {
            self.progress();
            self.dispatch();
            if self.stopping && self.workers.iter().all(Option::is_none) {
                self.jobs.clear();
                for reply in self.shutdown_replies {
                    let _ = reply.send(());
                }
                return;
            }
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(command) => self.command(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.stop(None),
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Submit { request, reply } => {
                let result = self.submit(*request);
                let _ = reply.send(result);
            }
            Command::Status { id, reply } => {
                let result = self
                    .jobs
                    .get(&id)
                    .map(|entry| entry.snapshot.clone())
                    .ok_or_else(|| task_error(TaskErrorKind::NotFound, Some(id)));
                let _ = reply.send(result);
            }
            Command::Cancel { id, reply } => {
                let _ = reply.send(self.cancel(id));
            }
            Command::Result { id, reply } => {
                let result = self.result(id);
                let _ = reply.send(result);
            }
            Command::Finished { worker, id, result } => {
                if let Some(slot) = self.workers.get_mut(worker).and_then(Option::as_mut) {
                    slot.active = None;
                }
                self.finish(id, *result);
            }
            Command::Stopped { worker, error } => {
                let active = self.workers[worker].take().and_then(|slot| slot.active);
                if let Some(id) = active {
                    self.finish(
                        id,
                        Err(error.as_deref().cloned().unwrap_or_else(|| {
                            worker_error(
                                WorkerErrorKind::Exit,
                                "analysis worker stopped before returning its task",
                            )
                        })),
                    );
                }
                if !self.stopping {
                    if error.as_ref().is_some_and(|error| matches!(&error.details, ErrorDetails::Worker(detail) if detail.kind == WorkerErrorKind::Spawn)) {
                        self.fail_waiting(*error.expect("spawn error was checked"));
                    } else {
                        match start_worker(worker, self.executor.clone(), self.sender.clone()) {
                            Ok(replacement) => self.workers[worker] = Some(replacement),
                            Err(error) => self.fail_waiting(worker_spawn_error(error)),
                        }
                    }
                }
            }
            Command::Shutdown { reply } => self.stop(reply),
        }
    }

    fn submit(&mut self, request: AnalyzeRequest) -> Result<JobSnapshot, ApiError> {
        if self.stopping {
            return Err(worker_error(
                WorkerErrorKind::Channel,
                "analysis pool is shutting down",
            ));
        }
        if self.workers.iter().all(Option::is_none) {
            return Err(worker_error(
                WorkerErrorKind::Exit,
                "no analysis workers remain",
            ));
        }
        let available = self
            .workers
            .iter()
            .flatten()
            .filter(|worker| worker.active.is_none() && worker.sender.is_some())
            .count();
        if self.waiting.len() >= self.config.queue_capacity.saturating_add(available) {
            return Err(task_error(TaskErrorKind::QueueFull, None));
        }
        let id = next_id()?;
        let (sender, events) = mpsc::sync_channel(32);
        let entry = Entry {
            snapshot: JobSnapshot {
                id,
                state: JobState::Queued,
                progress: AnalysisProgress::default(),
            },
            control: Control::new(Observer::new(sender)),
            events,
            request: Some(request),
            report: None,
        };
        self.jobs.insert(id, entry);
        self.waiting.push_back(id);
        self.dispatch();
        Ok(self.jobs[&id].snapshot.clone())
    }

    fn dispatch(&mut self) {
        if self.stopping {
            return;
        }
        for worker in 0..self.workers.len() {
            if !self.workers[worker]
                .as_ref()
                .is_some_and(|slot| slot.active.is_none() && slot.sender.is_some())
            {
                continue;
            }
            let Some(id) = self.waiting.pop_front() else {
                return;
            };
            let entry = self
                .jobs
                .get_mut(&id)
                .expect("queued job remains owned by coordinator");
            let request = entry.request.take().expect("queued job has one request");
            entry.snapshot.state = JobState::Running;
            entry.snapshot.progress.phase = AnalysisPhase::Validating;
            let work = Work {
                id,
                request,
                control: entry.control.clone(),
            };
            let slot = self.workers[worker].as_mut().expect("worker checked above");
            slot.active = Some(id);
            if slot
                .sender
                .as_ref()
                .expect("available sender checked above")
                .send(work)
                .is_err()
            {
                // Wait for the joined-worker Stopped event before acknowledging
                // teardown, even when its input channel has already closed.
                slot.sender.take();
            }
        }
    }

    fn result(&self, id: JobId) -> Result<Arc<AnalysisReport>, ApiError> {
        let entry = self
            .jobs
            .get(&id)
            .ok_or_else(|| task_error(TaskErrorKind::NotFound, Some(id)))?;
        match &entry.snapshot.state {
            JobState::Completed => Ok(Arc::clone(
                entry.report.as_ref().expect("completed job owns report"),
            )),
            JobState::Failed(error) => Err(error.clone()),
            JobState::Cancelled => Err(task_error(TaskErrorKind::Cancelled, Some(id))),
            JobState::Queued | JobState::Running | JobState::Cancelling => {
                Err(task_error(TaskErrorKind::NotReady, Some(id)))
            }
        }
    }

    fn cancel(&mut self, id: JobId) -> Result<JobSnapshot, ApiError> {
        let entry = self
            .jobs
            .get_mut(&id)
            .ok_or_else(|| task_error(TaskErrorKind::NotFound, Some(id)))?;
        match entry.snapshot.state {
            JobState::Queued => {
                entry.control.cancellation().cancel();
                entry.request.take();
                self.waiting.retain(|waiting| *waiting != id);
                self.finish(id, Err(task_error(TaskErrorKind::Cancelled, Some(id))));
            }
            JobState::Running => {
                entry.control.cancellation().cancel();
                entry.snapshot.state = JobState::Cancelling;
            }
            JobState::Cancelling
            | JobState::Completed
            | JobState::Failed(_)
            | JobState::Cancelled => {}
        }
        Ok(self.jobs[&id].snapshot.clone())
    }

    fn finish(&mut self, id: JobId, result: Result<AnalysisReport, ApiError>) {
        let Some(entry) = self.jobs.get_mut(&id) else {
            return;
        };
        drain_progress(entry);
        if entry.control.cancellation().is_cancelled() {
            drop(result);
            entry.snapshot.state = JobState::Cancelled;
            entry.snapshot.progress.phase = AnalysisPhase::Cancelled;
        } else {
            match result {
                Ok(report) => {
                    entry.snapshot.progress.phase = AnalysisPhase::Complete;
                    entry.snapshot.progress.states =
                        entry.snapshot.progress.states.max(report.cfg.len());
                    entry.snapshot.progress.transfers =
                        entry.snapshot.progress.transfers.max(report.transfers);
                    entry.snapshot.progress.work = report.metadata.work;
                    entry.snapshot.progress.accounts = report.accounts.len();
                    entry.snapshot.progress.slots = report
                        .accounts
                        .iter()
                        .map(|account| account.storage.len())
                        .sum();
                    if let Some(acquisition) = &report.acquisition {
                        entry.snapshot.progress.round = acquisition.rounds;
                        entry.snapshot.progress.requests = acquisition.requests;
                        entry.snapshot.progress.states = entry
                            .snapshot
                            .progress
                            .states
                            .max(acquisition.states_created);
                    }
                    entry.report = Some(Arc::new(report));
                    entry.snapshot.state = JobState::Completed;
                }
                Err(error) => entry.snapshot.state = JobState::Failed(error),
            }
        }
        self.retained.push_back(id);
        while self.retained.len() > self.config.retained_jobs {
            if let Some(expired) = self.retained.pop_front() {
                self.jobs.remove(&expired);
            }
        }
    }

    fn fail_waiting(&mut self, error: ApiError) {
        while let Some(id) = self.waiting.pop_front() {
            if let Some(entry) = self.jobs.get_mut(&id) {
                entry.request.take();
            }
            self.finish(id, Err(error.clone()));
        }
    }

    fn progress(&mut self) {
        for entry in self.jobs.values_mut() {
            if matches!(
                entry.snapshot.state,
                JobState::Running | JobState::Cancelling
            ) {
                drain_progress(entry);
            }
        }
    }

    fn stop(&mut self, reply: Option<SyncSender<()>>) {
        if let Some(reply) = reply {
            self.shutdown_replies.push(reply);
        }
        self.stopping = true;
        for worker in self.workers.iter_mut().flatten() {
            worker.sender.take();
        }
        for entry in self.jobs.values_mut() {
            if matches!(
                entry.snapshot.state,
                JobState::Queued | JobState::Running | JobState::Cancelling
            ) {
                entry.control.cancellation().cancel();
                if entry.snapshot.state == JobState::Running {
                    entry.snapshot.state = JobState::Cancelling;
                }
            }
        }
        while let Some(id) = self.waiting.pop_front() {
            if let Some(entry) = self.jobs.get_mut(&id) {
                entry.request.take();
            }
            self.finish(id, Err(task_error(TaskErrorKind::Cancelled, Some(id))));
        }
    }
}

fn drain_progress(entry: &mut Entry) {
    for event in entry.events.try_iter() {
        let progress = &mut entry.snapshot.progress;
        match event {
            Event::Phase(phase) => {
                progress.phase = match phase {
                    Phase::Validating => AnalysisPhase::Validating,
                    Phase::Pinning => AnalysisPhase::Pinning,
                    Phase::Acquiring => AnalysisPhase::Acquiring,
                    Phase::Analyzing => AnalysisPhase::Analyzing,
                    Phase::BuildingSsa => AnalysisPhase::BuildingSsa,
                    Phase::Projecting => AnalysisPhase::Projecting,
                }
            }
            Event::Execution {
                states,
                transfers,
                work,
            } => {
                progress.states = progress.states.max(states);
                progress.transfers = progress.transfers.max(transfers as u64);
                progress.work = progress.work.max(work as u64);
            }
            Event::Acquisition {
                round,
                accounts,
                slots,
                requests,
            } => {
                progress.round = progress.round.max(round);
                progress.accounts = progress.accounts.max(accounts);
                progress.slots = progress.slots.max(slots);
                progress.requests = progress.requests.max(requests);
            }
        }
    }
}

fn task_error(kind: TaskErrorKind, id: Option<JobId>) -> ApiError {
    let (code, message) = match kind {
        TaskErrorKind::NotFound => (
            ApiErrorCode::TaskNotFound,
            "analysis task not found or expired",
        ),
        TaskErrorKind::NotReady => (ApiErrorCode::TaskNotReady, "analysis result is not ready"),
        TaskErrorKind::QueueFull => (ApiErrorCode::QueueFull, "analysis queue is full"),
        TaskErrorKind::Cancelled => (ApiErrorCode::Cancelled, "analysis was cancelled"),
    };
    ApiError {
        code,
        message: message.into(),
        details: ErrorDetails::Task(TaskFailure { kind, id }),
    }
}

struct CompletionGuard {
    sender: SyncSender<Command>,
    worker: usize,
    failure: Option<Box<ApiError>>,
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        let error = if thread::panicking() {
            Some(Box::new(worker_error(
                WorkerErrorKind::Panic,
                "analysis worker panicked",
            )))
        } else {
            self.failure.take()
        };
        let _ = self.sender.send(Command::Stopped {
            worker: self.worker,
            error,
        });
    }
}

fn start_worker<E: Executor>(
    worker: usize,
    executor: E,
    sender: SyncSender<Command>,
) -> Result<Worker, std::io::Error> {
    let (work_sender, receiver) = mpsc::sync_channel::<Work>(1);
    thread::Builder::new()
        .name(format!("analysis-owner-{worker}"))
        .spawn(move || {
            // scope joins the analysis thread, including TLS destructors, before
            // this outer guard acknowledges exit. It also propagates a worker panic
            // without exposing JoinHandle's dynamically typed panic payload.
            let mut guard = CompletionGuard {
                sender,
                worker,
                failure: None,
            };
            thread::scope(|scope| {
                let events = guard.sender.clone();
                if let Err(error) = thread::Builder::new()
                    .name(format!("analysis-{worker}"))
                    .spawn_scoped(scope, move || {
                        while let Ok(work) = receiver.recv() {
                            let result = executor.execute(work.request, &work.control);
                            let id = work.id;
                            drop(work.control);
                            if events
                                .send(Command::Finished {
                                    worker,
                                    id,
                                    result: Box::new(result),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                {
                    guard.failure = Some(Box::new(worker_spawn_error(error)));
                }
            });
        })?;
    Ok(Worker {
        sender: Some(work_sender),
        active: None,
    })
}
