//! Independent decompiler lifecycle. A generation qualifies every callback;
//! worker cleanup and immutable result delivery remain separate transitions.

use crate::{JobOperation, TransportError};
use evm_abstract_protocol::{
    ApiError, DecompileJobReply, DecompileJobResultReply, DecompileJobSnapshot, DecompileRequest,
    JobId, JobState, PublicationVersion,
};

pub(crate) enum Command {
    Submit {
        generation: u64,
        request: Box<DecompileRequest>,
    },
    Poll {
        generation: u64,
        id: JobId,
    },
    Cancel {
        generation: u64,
        id: JobId,
    },
    Result {
        generation: u64,
        id: JobId,
    },
}

pub(crate) enum Message {
    Status {
        generation: u64,
        operation: JobOperation,
        result: Box<Result<DecompileJobReply, TransportError>>,
    },
    Result {
        generation: u64,
        id: JobId,
        result: Box<Result<DecompileJobResultReply, TransportError>>,
    },
}

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("{0}")]
    Transport(#[from] TransportError),
    #[error("{code:?}: {message}", code = .0.code, message = .0.message)]
    Api(ApiError),
    #[error("Task response identity mismatch: expected {expected}, received {received}")]
    Identity { expected: JobId, received: JobId },
    #[error("Result publication is older than the advertised publication")]
    Publication,
    #[error("Invalid decompiler artifact: {0}")]
    Artifact(String),
    #[error("Attempt {completion:?} has no validated stage; previous artifact retained. {message}")]
    UnvalidatedAttempt {
        completion: evm_abstract_protocol::decompile_model::Completion,
        message: String,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Submitting,
    Active,
    Terminal,
    Missing,
}

#[derive(Default)]
pub(super) struct Task {
    generation: u64,
    phase: Phase,
    pub snapshot: Option<DecompileJobSnapshot>,
    pub error: Option<Error>,
    delivered: Option<PublicationVersion>,
    rejected: Option<PublicationVersion>,
    status_pending: bool,
    result_pending: bool,
    cancel_requested: bool,
    cancel_pending: bool,
    next_poll: f64,
}

impl Task {
    pub fn busy(&self) -> bool {
        self.phase == Phase::Submitting || self.phase == Phase::Active
    }

    pub fn start(&mut self, request: DecompileRequest) -> Command {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("decompiler generation exhausted");
        self.phase = Phase::Submitting;
        self.snapshot = None;
        self.error = None;
        self.delivered = None;
        self.rejected = None;
        self.status_pending = true;
        self.result_pending = false;
        self.cancel_requested = false;
        self.cancel_pending = false;
        self.next_poll = 0.0;
        Command::Submit {
            generation: self.generation,
            request: Box::new(request),
        }
    }

    pub fn request_cancel(&mut self) {
        if self.busy() {
            self.cancel_requested = true;
        }
    }

    pub fn next(&mut self, now: f64) -> Option<Command> {
        if self.phase == Phase::Missing {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot.id;
        let generation = self.generation;
        if self.cancel_requested
            && !self.cancel_pending
            && !matches!(snapshot.state, JobState::Cancelling)
            && self.busy()
        {
            self.cancel_pending = true;
            return Some(Command::Cancel { generation, id });
        }
        if now < self.next_poll {
            return None;
        }
        if !self.result_pending
            && snapshot.publication.is_some_and(|version| {
                self.delivered.is_none_or(|old| old < version)
                    && self.rejected.is_none_or(|old| old < version)
            })
        {
            self.result_pending = true;
            self.next_poll = now + 0.35;
            return Some(Command::Result { generation, id });
        }
        if self.busy() && !self.status_pending {
            self.status_pending = true;
            self.next_poll = now + 0.35;
            return Some(Command::Poll { generation, id });
        }
        None
    }

    pub fn needs_refresh(&self) -> bool {
        if self.phase == Phase::Missing {
            return self.result_pending;
        }
        self.busy()
            || self.result_pending
            || self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.publication.is_some_and(|version| {
                    self.delivered.is_none_or(|old| old < version)
                        && self.rejected.is_none_or(|old| old < version)
                })
            })
    }

    pub fn receive_status(
        &mut self,
        generation: u64,
        operation: JobOperation,
        result: Result<DecompileJobReply, TransportError>,
    ) {
        if generation != self.generation {
            return;
        }
        if operation == JobOperation::Cancel {
            self.cancel_pending = false;
        } else {
            self.status_pending = false;
        }
        let result = result
            .map_err(Error::Transport)
            .and_then(|reply| reply.result.map_err(Error::Api));
        let mut next = match result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if operation == JobOperation::Cancel {
                    self.cancel_requested = false;
                }
                if matches!(&error, Error::Api(error) if error.code == evm_abstract_protocol::ApiErrorCode::TaskNotFound)
                {
                    // A qualified result already in flight may still arrive.
                    // Retire future polling/cancellation without erasing it.
                    self.phase = Phase::Missing;
                    self.cancel_requested = false;
                } else if self.snapshot.is_none() {
                    self.phase = Phase::Terminal;
                }
                self.error = Some(error);
                return;
            }
        };
        if self.phase == Phase::Missing {
            return;
        }
        if let Some(previous) = &self.snapshot {
            if next.id != previous.id {
                self.error = Some(Error::Identity {
                    expected: previous.id,
                    received: next.id,
                });
                return;
            }
            if terminal(&previous.state) && next.state != previous.state {
                return;
            }
            if previous.state == JobState::Cancelling
                && matches!(next.state, JobState::Queued | JobState::Running)
            {
                return;
            }
            if next.publication < previous.publication {
                next.publication = previous.publication;
            }
            if next.progress.work < previous.progress.work {
                next.progress = previous.progress.clone();
            }
            next.queue_ns = next.queue_ns.max(previous.queue_ns);
            next.processing_ns = next.processing_ns.max(previous.processing_ns);
        }
        self.phase = if terminal(&next.state) {
            Phase::Terminal
        } else {
            Phase::Active
        };
        if let JobState::Failed(error) = &next.state {
            self.error = Some(Error::Api(error.clone()));
        } else if next.state == JobState::Completed && next.publication.is_none() {
            self.error = Some(Error::Artifact("completed task has no publication".into()));
        } else if self.rejected.is_none() {
            self.error = None;
        }
        self.snapshot = Some(next);
    }

    pub fn accepts_result(&self, generation: u64, id: JobId) -> bool {
        generation == self.generation
            && self.result_pending
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.id == id)
    }

    pub fn accept_publication(&self, publication: PublicationVersion) -> bool {
        self.delivered.is_none_or(|old| old < publication)
            && self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.publication)
                .is_none_or(|advertised| advertised <= publication)
    }

    pub fn delivered(&mut self, publication: PublicationVersion) {
        self.delivered = Some(publication);
        self.rejected = None;
        self.result_pending = false;
        if self.phase != Phase::Missing
            && self
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| !matches!(snapshot.state, JobState::Failed(_)))
        {
            self.error = None;
        }
    }

    pub fn result_failed(&mut self, error: Error) {
        self.result_pending = false;
        self.error = Some(error);
    }

    pub fn reject(&mut self, publication: PublicationVersion, error: Error) {
        self.rejected = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.publication)
            .or(Some(publication));
        self.result_failed(error);
    }

    pub fn label(&self) -> &'static str {
        if self.phase == Phase::Missing {
            return "Task unavailable";
        }
        if self.phase == Phase::Submitting {
            return if self.cancel_requested {
                "Cancelling after admission"
            } else {
                "Submitting"
            };
        }
        if self.cancel_requested && self.busy() {
            return "Cancelling — waiting for worker cleanup";
        }
        match self.snapshot.as_ref().map(|snapshot| &snapshot.state) {
            None if self.error.is_some() => "Failed",
            None => "Ready to decompile",
            Some(JobState::Queued) => "Queued",
            Some(JobState::Running) => "Running",
            Some(JobState::Cancelling) => "Cancelling — waiting for worker cleanup",
            Some(JobState::Cancelled) => "Cancelled",
            Some(JobState::Failed(_)) => "Failed",
            Some(JobState::Completed) if self.error.is_some() => "Result error",
            Some(JobState::Completed) if self.delivered.is_some() => "Ready",
            Some(JobState::Completed) => "Loading result",
        }
    }
}

fn terminal(state: &JobState) -> bool {
    matches!(
        state,
        JobState::Completed | JobState::Cancelled | JobState::Failed(_)
    )
}
