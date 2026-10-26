//! Client task lifecycle. Generation and job checks keep late poll/result
//! callbacks from replacing a newer immutable report.

use super::{TransportError, timing};
pub(super) mod presentation;
use evm_abstract_notation::WideSymbol;
use evm_abstract_protocol::{
    AnalyzeRequest, ApiError, JobId, JobPublication, JobReply, JobResultReply, JobSnapshot,
    JobState, PublicationVersion, RpcProvidersReply,
};
use presentation::TaskIntent;

/// The HTTP status operation that produced a task snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobOperation {
    /// Create a task.
    Submit,
    /// Read its latest state and progress.
    Poll,
    /// Request cooperative cancellation.
    Cancel,
}

/// Work requested by the pure UI; the framework executes the HTTP operation.
pub enum Command {
    /// Read the server-owned RPC provider catalog.
    RpcProviders {
        /// Catalog generation, independent from analysis task generations.
        generation: u64,
    },
    /// Submit a new immutable analysis request.
    Submit {
        /// UI generation, echoed into the callback.
        generation: u64,
        /// Typed request body.
        request: Box<AnalyzeRequest>,
    },
    /// Read status without downloading the report.
    Poll {
        /// UI generation, echoed into the callback.
        generation: u64,
        /// Backend task identity.
        id: JobId,
    },
    /// Download the latest advertised immutable publication.
    Result {
        /// UI generation, echoed into the callback.
        generation: u64,
        /// Backend task identity.
        id: JobId,
    },
    /// Ask the backend to stop and acknowledge worker cleanup.
    Cancel {
        /// UI generation, echoed into the callback.
        generation: u64,
        /// Backend task identity.
        id: JobId,
    },
}

/// Typed asynchronous callback consumed by the browser-independent workspace.
pub enum Message {
    /// RPC catalog response, independent from any analysis result.
    RpcProviders {
        /// Generation from the originating catalog command.
        generation: u64,
        /// Public provider names and IDs, or a transport failure.
        result: Result<RpcProvidersReply, TransportError>,
    },
    /// Result of task submission, polling, or cancellation.
    Status {
        /// Generation from the originating command.
        generation: u64,
        /// Operation used for retry and cancellation handling.
        operation: JobOperation,
        /// Typed backend envelope or transport failure.
        result: Box<Result<JobReply, TransportError>>,
    },
    /// Version-qualified immutable analysis publication.
    Result {
        /// Generation from the originating command.
        generation: u64,
        /// Backend task identity.
        id: JobId,
        /// Typed report envelope or transport failure.
        result: Box<Result<JobResultReply, TransportError>>,
        /// Network and body-transfer duration of this result attempt.
        transfer_ns: u64,
        /// JSON decoding duration of this result attempt.
        parsing_ns: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TaskPhase {
    #[default]
    Idle,
    Submitting,
    Queued,
    Running,
    Cancelling,
    Fetching,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug)]
pub(super) enum TaskError {
    Api(ApiError),
    Transport(TransportError),
    Schema { received: u16 },
    Identity { expected: JobId, received: JobId },
    Publication,
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Publication => f.write_str("Invalid result publication: graph versions do not match the report or advertised identity"),
            Self::Api(error) => write!(f, "{:?}: {}", error.code, error.message),
            Self::Transport(error) => error.fmt(f),
            Self::Schema { received } => write!(
                f,
                "Unsupported response schema {received}; expected {}",
                evm_abstract_protocol::SCHEMA_VERSION
            ),
            Self::Identity { expected, received } => write!(
                f,
                "Task response identity mismatch: expected {}, received {}",
                WideSymbol::Task(expected.0),
                WideSymbol::Task(received.0)
            ),
        }
    }
}

#[derive(Default)]
pub(super) struct Task {
    pub(super) phase: TaskPhase,
    pub(super) snapshot: Option<JobSnapshot>,
    pub(super) error: Option<TaskError>,
    generation: u64,
    poll_pending: bool,
    result_pending: bool,
    delivered: Option<JobPublication>,
    rejected: Option<PublicationVersion>,
    cancel_requested: bool,
    cancel_sent: bool,
    next_poll: Option<f64>,
    pub(super) wait: timing::Wait,
    pub(super) frontend: timing::Frontend,
}

impl Task {
    pub(super) fn busy(&self) -> bool {
        matches!(
            self.phase,
            TaskPhase::Submitting
                | TaskPhase::Queued
                | TaskPhase::Running
                | TaskPhase::Cancelling
                | TaskPhase::Fetching
        )
    }

    pub(super) fn cancellable(&self) -> bool {
        matches!(
            self.phase,
            TaskPhase::Submitting | TaskPhase::Queued | TaskPhase::Running
        )
    }

    pub(super) fn start(&mut self, request: AnalyzeRequest) -> Command {
        let generation = self
            .generation
            .checked_add(1)
            .expect("task generation exhausted");
        *self = Self {
            generation,
            phase: TaskPhase::Submitting,
            ..Self::default()
        };
        self.wait.start(timing::now());
        Command::Submit {
            generation,
            request: Box::new(request),
        }
    }

    pub(super) fn cancel(&mut self) -> Option<Command> {
        self.apply_intent(TaskIntent::Cancel)
    }

    pub(super) fn apply_intent(&mut self, intent: TaskIntent) -> Option<Command> {
        let TaskIntent::Cancel = intent;
        if !self.cancellable() {
            return None;
        }
        self.cancel_requested = true;
        self.phase = TaskPhase::Cancelling;
        self.cancel_command()
    }

    fn cancel_command(&mut self) -> Option<Command> {
        if self.cancel_requested && !self.cancel_sent {
            let id = self.snapshot.as_ref()?.id;
            if !matches!(
                self.snapshot.as_ref()?.state,
                JobState::Queued | JobState::Running
            ) {
                return None;
            }
            self.cancel_sent = true;
            Some(Command::Cancel {
                generation: self.generation,
                id,
            })
        } else {
            None
        }
    }

    fn unpublished(&self) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.publication.as_ref())
            .is_some_and(|publication| {
                self.delivered
                    .as_ref()
                    .is_none_or(|old| publication.version > old.version)
                    && self
                        .rejected
                        .is_none_or(|rejected| publication.version > rejected)
            })
    }

    pub(super) fn next_command(&mut self, now: f64) -> Option<Command> {
        // Standalone replacement retires transport even if its old snapshot
        // advertised a publication that was still in flight.
        if self.phase == TaskPhase::Ready {
            return None;
        }
        if let Some(command) = self.cancel_command() {
            return Some(command);
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot.id;
        let unpublished = self.unpublished();
        // Retained closed models remain fetchable after cancellation/failure.
        // A staged download leaves Running cancellable and status polling active.
        if unpublished && !self.result_pending {
            if self.error.is_some() && now < *self.next_poll.get_or_insert(now + 1.0) {
                return None;
            }
            self.next_poll = None;
            self.result_pending = true;
            return Some(Command::Result {
                generation: self.generation,
                id,
            });
        }
        if !self.busy() || self.poll_pending || snapshot.state == JobState::Completed {
            return None;
        }
        let delay = if self.error.is_some() { 1.0 } else { 0.3 };
        let due = self.next_poll.get_or_insert(now + delay);
        if now < *due {
            return None;
        }
        self.poll_pending = true;
        self.next_poll = None;
        Some(Command::Poll {
            generation: self.generation,
            id,
        })
    }

    pub(super) fn receive_status(
        &mut self,
        generation: u64,
        operation: JobOperation,
        result: Result<JobReply, TransportError>,
    ) {
        if generation != self.generation {
            return;
        }
        if operation == JobOperation::Poll {
            self.poll_pending = false;
        }
        if matches!(
            self.phase,
            TaskPhase::Ready | TaskPhase::Cancelled | TaskPhase::Failed
        ) {
            return;
        }
        let mut snapshot = match result {
            Ok(JobReply {
                result: Ok(snapshot),
            }) => snapshot,
            Ok(JobReply { result: Err(error) }) => {
                if error.code == evm_abstract_protocol::ApiErrorCode::TaskNotFound
                    && !self.result_pending
                {
                    self.fail(TaskError::Api(error));
                } else {
                    self.request_failed(operation, TaskError::Api(error));
                }
                return;
            }
            Err(error) => {
                self.request_failed(operation, TaskError::Transport(error));
                return;
            }
        };
        if let Some(current) = &self.snapshot {
            if current.id != snapshot.id {
                self.request_failed(
                    operation,
                    TaskError::Identity {
                        expected: current.id,
                        received: snapshot.id,
                    },
                );
                return;
            }
            if snapshot.publication.as_ref().is_none_or(|next| {
                current
                    .publication
                    .as_ref()
                    .is_some_and(|old| old.version > next.version)
            }) {
                snapshot.publication = current.publication.clone();
            }
            if current.state == JobState::Completed && snapshot.state != JobState::Completed {
                return;
            }
            if current.state == JobState::Cancelling
                && matches!(snapshot.state, JobState::Queued | JobState::Running)
            {
                return;
            }
        }
        let rejected = snapshot.publication.as_ref().is_some_and(|publication| {
            self.rejected
                .is_some_and(|rejected| publication.version <= rejected)
        });
        if !rejected {
            self.error = None;
        }
        self.phase = match &snapshot.state {
            JobState::Queued if !self.cancel_requested => TaskPhase::Queued,
            JobState::Running if !self.cancel_requested => TaskPhase::Running,
            JobState::Queued | JobState::Running | JobState::Cancelling => TaskPhase::Cancelling,
            JobState::Completed if rejected => TaskPhase::Failed,
            JobState::Completed
                if snapshot.publication.as_ref().is_some_and(|publication| {
                    self.delivered
                        .as_ref()
                        .is_some_and(|delivered| delivered.version >= publication.version)
                }) =>
            {
                TaskPhase::Ready
            }
            JobState::Completed => TaskPhase::Fetching,
            JobState::Cancelled => TaskPhase::Cancelled,
            JobState::Failed(error) => {
                self.error = Some(TaskError::Api(error.clone()));
                TaskPhase::Failed
            }
        };
        self.snapshot = Some(snapshot);
        self.next_poll = None;
        if matches!(self.phase, TaskPhase::Failed | TaskPhase::Cancelled) {
            self.wait.finish(timing::now());
        }
    }

    fn request_failed(&mut self, operation: JobOperation, error: TaskError) {
        let Some(snapshot) = &self.snapshot else {
            self.fail(error);
            return;
        };
        // A failed HTTP operation cannot establish that known native work ended.
        self.error = Some(error);
        self.next_poll = None;
        if operation == JobOperation::Cancel {
            self.cancel_requested = false;
            self.cancel_sent = false;
            self.phase = match &snapshot.state {
                JobState::Completed
                    if snapshot.publication.as_ref().is_some_and(|publication| {
                        self.delivered
                            .as_ref()
                            .is_some_and(|delivered| delivered.version >= publication.version)
                    }) =>
                {
                    TaskPhase::Ready
                }
                JobState::Completed => TaskPhase::Fetching,
                JobState::Cancelling => TaskPhase::Cancelling,
                JobState::Cancelled => TaskPhase::Cancelled,
                JobState::Failed(_) => TaskPhase::Failed,
                JobState::Queued => TaskPhase::Queued,
                JobState::Running => TaskPhase::Running,
            };
        }
    }

    pub(super) fn retry_result(&mut self, error: TaskError) {
        self.error = Some(error);
        self.result_pending = false;
        self.next_poll = None;
    }

    pub(super) fn accepts_result(&self, generation: u64, id: JobId) -> bool {
        generation == self.generation
            && self.result_pending
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.id == id)
    }

    pub(super) fn accepts_publication(&self, publication: &JobPublication) -> bool {
        self.delivered
            .as_ref()
            .is_none_or(|old| publication.version > old.version)
            && self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.publication.as_ref())
                .is_none_or(|advertised| publication.version >= advertised.version)
    }

    pub(super) fn advertised_publication(&self) -> Option<&JobPublication> {
        self.snapshot.as_ref()?.publication.as_ref()
    }

    pub(super) fn discard_result(&mut self) {
        self.result_pending = false;
        self.next_poll = None;
    }

    pub(super) fn published(&mut self, publication: JobPublication) {
        if let Some(snapshot) = &mut self.snapshot {
            if snapshot
                .publication
                .as_ref()
                .is_none_or(|old| old.version < publication.version)
            {
                snapshot.publication = Some(publication.clone());
            }
            if snapshot.state == JobState::Completed {
                self.phase = TaskPhase::Ready;
            }
        }
        self.delivered = Some(publication);
        self.rejected = None;
        self.result_pending = false;
        self.error = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| match &snapshot.state {
                JobState::Failed(error) => Some(TaskError::Api(error.clone())),
                _ => None,
            });
    }

    /// Invalid immutable contents cannot end known worker execution or cause an
    /// endless retry of the same publication. A later version can recover.
    pub(super) fn reject_result(&mut self, version: Option<PublicationVersion>, error: TaskError) {
        self.rejected = version.or_else(|| {
            self.advertised_publication()
                .map(|publication| publication.version)
        });
        self.discard_result();
        if self.snapshot.as_ref().is_some_and(|snapshot| {
            matches!(
                snapshot.state,
                JobState::Queued | JobState::Running | JobState::Cancelling
            )
        }) {
            self.error = Some(error);
        } else {
            self.fail(error);
        }
    }

    /// Retained publication downloads and retry deadlines outlive worker cleanup.
    pub(super) fn needs_refresh(&self) -> bool {
        self.phase != TaskPhase::Ready && (self.busy() || self.result_pending || self.unpublished())
    }

    pub(super) fn complete(&mut self) {
        self.phase = TaskPhase::Ready;
        self.result_pending = false;
        self.poll_pending = false;
        self.cancel_requested = false;
        self.cancel_sent = false;
        self.next_poll = None;
        self.error = None;
        // The workspace freezes successful waiting only after the actual
        // current view has completed its first synchronous preparation.
    }

    pub(super) fn fail(&mut self, error: TaskError) {
        self.error = Some(error);
        self.phase = TaskPhase::Failed;
        self.poll_pending = false;
        self.result_pending = false;
        self.wait.finish(timing::now());
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn label(&self) -> &'static str {
        match self.phase {
            TaskPhase::Idle => "Ready to analyze",
            TaskPhase::Submitting => "Submitting",
            TaskPhase::Queued => "Queued",
            TaskPhase::Running => "Running",
            TaskPhase::Cancelling => "Cancelling — waiting for worker cleanup",
            TaskPhase::Fetching => "Loading result",
            TaskPhase::Ready => "Ready",
            TaskPhase::Failed => "Failed",
            TaskPhase::Cancelled => "Cancelled",
        }
    }
}

#[cfg(test)]
mod tests;
