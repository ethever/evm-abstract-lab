use super::*;
use evm_abstract_protocol::AnalysisProgress;

fn task() -> Task {
    let mut task = Task::default();
    task.start(AnalyzeRequest::default());
    task
}
fn status(task: &mut Task, operation: JobOperation, state: JobState) {
    task.receive_status(
        1,
        operation,
        Ok(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(17),
                state,
                progress: AnalysisProgress::default(),
            }),
        }),
    );
}
fn disconnected() -> TransportError {
    TransportError::Browser {
        message: "network interrupted".into(),
    }
}

#[test]
fn cancellation_waits_for_worker_ack_and_late_running_poll_cannot_reopen_it() {
    let mut task = task();
    assert!(
        task.cancel().is_none(),
        "cancel before admission waits for the actual job id"
    );
    status(&mut task, JobOperation::Submit, JobState::Queued);
    assert!(matches!(
        task.next_command(0.0),
        Some(Command::Cancel { id: JobId(17), .. })
    ));
    status(&mut task, JobOperation::Cancel, JobState::Cancelling);
    status(&mut task, JobOperation::Poll, JobState::Running);
    assert_eq!(task.phase, TaskPhase::Cancelling);
    assert!(task.busy());
    assert!(task.next_command(0.0).is_none());
    assert!(matches!(task.next_command(0.4), Some(Command::Poll { .. })));
    status(&mut task, JobOperation::Poll, JobState::Cancelled);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert!(!task.busy());
    status(&mut task, JobOperation::Poll, JobState::Completed);
    assert_eq!(task.phase, TaskPhase::Cancelled);
}

#[test]
fn failed_cancel_transport_does_not_strand_a_concurrently_completed_result() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    assert!(matches!(task.cancel(), Some(Command::Cancel { .. })));
    status(&mut task, JobOperation::Poll, JobState::Completed);
    assert!(matches!(
        task.next_command(0.0),
        Some(Command::Result { .. })
    ));
    task.receive_status(1, JobOperation::Cancel, Err(disconnected()));
    assert_eq!(task.phase, TaskPhase::Fetching);
    assert!(task.accepts_result(1, JobId(17)));
    task.complete();
    assert_eq!(task.phase, TaskPhase::Ready);
}

#[test]
fn lost_cancel_response_retains_job_identity_and_allows_an_explicit_retry() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    task.cancel();
    task.receive_status(1, JobOperation::Cancel, Err(disconnected()));
    assert_eq!(task.phase, TaskPhase::Running);
    assert_eq!(task.snapshot.as_ref().unwrap().id, JobId(17));
    assert!(task.error.is_some());
    assert!(matches!(
        task.cancel(),
        Some(Command::Cancel { id: JobId(17), .. })
    ));
    status(&mut task, JobOperation::Cancel, JobState::Cancelling);
    assert!(task.error.is_none());
}

#[test]
fn superseded_and_wrong_identity_callbacks_never_replace_the_current_task() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    task.start(AnalyzeRequest::default());
    status(&mut task, JobOperation::Poll, JobState::Completed);
    assert_eq!(task.phase, TaskPhase::Submitting);
    assert!(task.snapshot.is_none());
    assert!(!task.accepts_result(1, JobId(17)));
    task.receive_status(
        2,
        JobOperation::Submit,
        Ok(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(18),
                state: JobState::Running,
                progress: AnalysisProgress::default(),
            }),
        }),
    );
    task.receive_status(
        2,
        JobOperation::Poll,
        Ok(JobReply {
            result: Ok(JobSnapshot {
                id: JobId(19),
                state: JobState::Completed,
                progress: AnalysisProgress::default(),
            }),
        }),
    );
    assert!(matches!(
        task.error,
        Some(TaskError::Identity {
            expected: JobId(18),
            received: JobId(19)
        })
    ));
    assert_eq!(task.snapshot.unwrap().id, JobId(18));
}

fn queue_full() -> ApiError {
    ApiError {
        code: evm_abstract_protocol::ApiErrorCode::QueueFull,
        message: "HTTP capacity temporarily exhausted".into(),
        details: evm_abstract_protocol::ErrorDetails::Task(evm_abstract_protocol::TaskFailure {
            kind: evm_abstract_protocol::TaskErrorKind::QueueFull,
            id: None,
        }),
    }
}

#[test]
fn typed_http_capacity_errors_preserve_running_jobs_and_completed_result_callbacks() {
    let mut task = task();
    status(&mut task, JobOperation::Submit, JobState::Running);
    task.receive_status(
        1,
        JobOperation::Poll,
        Ok(JobReply {
            result: Err(queue_full()),
        }),
    );
    assert!(task.busy() && task.cancellable());
    assert!(task.next_command(0.0).is_none());
    assert!(matches!(
        task.next_command(1.1),
        Some(Command::Poll { id: JobId(17), .. })
    ));
    task.cancel();
    status(&mut task, JobOperation::Poll, JobState::Completed);
    assert!(matches!(
        task.next_command(1.2),
        Some(Command::Result { .. })
    ));
    task.receive_status(
        1,
        JobOperation::Cancel,
        Ok(JobReply {
            result: Err(queue_full()),
        }),
    );
    assert!(task.accepts_result(1, JobId(17)));
    task.retry_result(TaskError::Transport(disconnected()));
    assert!(
        task.next_command(2.0).is_none(),
        "failed download retries must be paced"
    );
    assert!(
        matches!(
            task.next_command(3.1),
            Some(Command::Result { id: JobId(17), .. })
        ),
        "retry the immutable result, not analysis execution"
    );
    task.complete();
    assert_eq!(task.phase, TaskPhase::Ready);
}

#[test]
fn task_identity_error_formats_full_u64_handles_as_symbols() {
    let expected = evm_abstract_notation::WideSymbol::Task(u64::MAX).to_string();
    let received = evm_abstract_notation::WideSymbol::Task(u64::MAX - 1).to_string();
    let error = TaskError::Identity {
        expected: JobId(u64::MAX),
        received: JobId(u64::MAX - 1),
    };
    assert_eq!(
        error.to_string(),
        format!("Task response identity mismatch: expected {expected}, received {received}")
    );
}
