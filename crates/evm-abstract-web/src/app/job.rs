//! Client task lifecycle. Generation and job checks keep late poll/result
//! callbacks from replacing a newer immutable report.

use super::TransportError;
use evm_abstract_protocol::{
    AnalyzeReply, AnalyzeRequest, ApiError, JobId, JobReply, JobSnapshot, JobState,
};

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
    /// Download the completed report.
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
    /// Result of task submission, polling, or cancellation.
    Status {
        /// Generation from the originating command.
        generation: u64,
        /// Operation used for retry and cancellation handling.
        operation: JobOperation,
        /// Typed backend envelope or transport failure.
        result: Result<JobReply, TransportError>,
    },
    /// Completed immutable analysis report.
    Result {
        /// Generation from the originating command.
        generation: u64,
        /// Backend task identity.
        id: JobId,
        /// Typed report envelope or transport failure.
        result: Box<Result<AnalyzeReply, TransportError>>,
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
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(error) => write!(f, "{:?}: {}", error.code, error.message),
            Self::Transport(error) => error.fmt(f),
            Self::Schema { received } => write!(
                f,
                "Unsupported response schema {received}; expected {}",
                evm_abstract_protocol::SCHEMA_VERSION
            ),
            Self::Identity { expected, received } => write!(
                f,
                "Task response identity mismatch: expected {expected}, received {received}"
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
    cancel_requested: bool,
    cancel_sent: bool,
    next_poll: Option<f64>,
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
        let generation = self.generation + 1;
        *self = Self {
            generation,
            phase: TaskPhase::Submitting,
            ..Self::default()
        };
        Command::Submit {
            generation,
            request: Box::new(request),
        }
    }

    pub(super) fn cancel(&mut self) -> Option<Command> {
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

    pub(super) fn next_command(&mut self, now: f64) -> Option<Command> {
        if !self.busy() {
            return None;
        }
        if let Some(command) = self.cancel_command() {
            return Some(command);
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot.id;
        if snapshot.state == JobState::Completed && !self.result_pending {
            if self.error.is_some() && now < *self.next_poll.get_or_insert(now + 1.0) {
                return None;
            }
            self.next_poll = None;
            self.result_pending = true;
            self.phase = TaskPhase::Fetching;
            return Some(Command::Result {
                generation: self.generation,
                id,
            });
        }
        if self.poll_pending || self.result_pending {
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
        let snapshot = match result {
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
                self.fail(TaskError::Identity {
                    expected: current.id,
                    received: snapshot.id,
                });
                return;
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
        self.error = None;
        self.phase = match &snapshot.state {
            JobState::Queued if !self.cancel_requested => TaskPhase::Queued,
            JobState::Running if !self.cancel_requested => TaskPhase::Running,
            JobState::Queued | JobState::Running | JobState::Cancelling => TaskPhase::Cancelling,
            JobState::Completed => TaskPhase::Fetching,
            JobState::Cancelled => TaskPhase::Cancelled,
            JobState::Failed(error) => {
                self.error = Some(TaskError::Api(error.clone()));
                TaskPhase::Failed
            }
        };
        self.snapshot = Some(snapshot);
        self.next_poll = None;
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
            && self.phase == TaskPhase::Fetching
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.id == id && snapshot.state == JobState::Completed)
    }

    pub(super) fn complete(&mut self) {
        self.phase = TaskPhase::Ready;
        self.result_pending = false;
        self.error = None;
    }

    pub(super) fn fail(&mut self, error: TaskError) {
        self.error = Some(error);
        self.phase = TaskPhase::Failed;
        self.poll_pending = false;
        self.result_pending = false;
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
