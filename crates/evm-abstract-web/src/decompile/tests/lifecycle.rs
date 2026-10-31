use super::{Command, JobOperation, Message, Strategy, Workspace, api, deliver, fixture, start};

fn missing() -> api::DecompileJobReply {
    api::DecompileJobReply {
        result: Err(api::ApiError {
            code: api::ApiErrorCode::TaskNotFound,
            message: "The server no longer retains this task".into(),
            details: api::ErrorDetails::Task(api::TaskFailure {
                kind: api::TaskErrorKind::NotFound,
                id: Some(api::JobId(11)),
            }),
        }),
    }
}

#[test]
fn missing_admitted_tasks_unlock_submission_and_preserve_the_last_occurrence() {
    for state in [api::JobState::Queued, api::JobState::Running] {
        for operation in [JobOperation::Poll, JobOperation::Cancel] {
            let mut workspace = Workspace::default();
            let artifact = fixture::artifact(Strategy::Dispatcher);
            deliver(&mut workspace, artifact.clone(), 10);
            let selection = workspace.state.selected.clone();
            let generation = start(&mut workspace);
            workspace.receive(Message::Status {
                generation,
                operation: JobOperation::Submit,
                result: Box::new(Ok(fixture::snapshot(11, state.clone(), None))),
            });
            assert!(workspace.task.busy());
            workspace.receive(Message::Status {
                generation,
                operation,
                result: Box::new(Ok(missing())),
            });
            assert!(!workspace.task.busy());
            assert!(!workspace.task.needs_refresh());
            assert!(workspace.task.next(100.0).is_none());
            assert_eq!(workspace.task.label(), "Task unavailable");
            assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
            assert_eq!(workspace.state.selected, selection);
            let next = start(&mut workspace);
            assert!(next > generation);
            assert!(workspace.task.busy());
            assert!(workspace.task.error.is_none());
        }
    }
}

#[test]
fn temporary_transport_failure_keeps_polling_and_missing_task_keeps_an_inflight_result() {
    let mut workspace = Workspace::default();
    let generation = start(&mut workspace);
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Box::new(Ok(fixture::snapshot(12, api::JobState::Running, None))),
    });
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Err(crate::TransportError::Http { status: 503 })),
    });
    assert!(workspace.task.busy());
    assert!(matches!(
        workspace.task.next(1.0),
        Some(Command::Poll { .. })
    ));
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Ok(fixture::snapshot(
            12,
            api::JobState::Running,
            Some(fixture::version(1)),
        ))),
    });
    assert!(matches!(
        workspace.task.next(2.0),
        Some(Command::Result { .. })
    ));
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Cancel,
        result: Box::new(Ok(missing())),
    });
    assert!(!workspace.task.busy());
    assert!(workspace.task.accepts_result(generation, api::JobId(12)));
    let artifact = fixture::artifact(Strategy::Dispatcher);
    workspace.receive(Message::Result {
        generation,
        id: api::JobId(12),
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id: api::JobId(12),
                publication: fixture::version(1),
                artifact: artifact.clone(),
            }),
        })),
    });
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
    assert_eq!(workspace.task.label(), "Task unavailable");
    assert!(!workspace.task.needs_refresh());
    assert!(workspace.task.error.is_some());
}
