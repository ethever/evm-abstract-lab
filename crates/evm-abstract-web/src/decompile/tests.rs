use super::{
    Workspace,
    diff::{Differences, Marker},
    model,
    task::{self, Command, Message},
};
use crate::JobOperation;
use evm_abstract_protocol::{
    self as api,
    decompile_model::{
        Artifact, Completion, Effects, EntityId, EntityRef, ExitKind, SourceSpan, StageId,
        Strategy, StructuredKind, StructuredNode, StructuredProgram, Terminator, ValidationStatus,
    },
};

mod fixture;

fn start(workspace: &mut Workspace) -> u64 {
    match workspace.task.start(api::DecompileRequest::default()) {
        Command::Submit {
            generation,
            request,
        } => {
            assert_eq!(request.strategy, Strategy::Dispatcher);
            generation
        }
        _ => unreachable!(),
    }
}

fn deliver(workspace: &mut Workspace, artifact: Artifact, job: u64) -> u64 {
    let generation = start(workspace);
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Box::new(Ok(fixture::snapshot(
            job,
            api::JobState::Completed,
            Some(fixture::version(1)),
        ))),
    });
    assert!(
        matches!(workspace.task.next(0.0), Some(Command::Result { id, generation: current }) if id == api::JobId(job) && current == generation)
    );
    workspace.receive(Message::Result {
        generation,
        id: api::JobId(job),
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id: api::JobId(job),
                publication: fixture::version(1),
                artifact,
            }),
        })),
    });
    generation
}

#[test]
fn late_results_and_rejected_publications_keep_the_last_valid_artifact() {
    let mut workspace = Workspace::default();
    let first = fixture::artifact(Strategy::Dispatcher);
    let old_generation = deliver(&mut workspace, first.clone(), 10);
    assert!(
        workspace
            .accessible_status()
            .contains("Decompile task: Ready")
    );
    let generation = start(&mut workspace);
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Box::new(Ok(fixture::snapshot(
            11,
            api::JobState::Completed,
            Some(fixture::version(1)),
        ))),
    });
    assert!(matches!(
        workspace.task.next(1.0),
        Some(Command::Result { .. })
    ));
    let mut invalid = fixture::artifact(Strategy::CodeDuplication);
    invalid.current_stage = Some(StageId(999));
    workspace.receive(Message::Result {
        generation,
        id: api::JobId(11),
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id: api::JobId(11),
                publication: fixture::version(1),
                artifact: invalid,
            }),
        })),
    });
    assert_eq!(workspace.artifact.as_ref(), Some(&first));
    assert!(
        workspace.task.next(20.0).is_none(),
        "invalid immutable result must not retry forever"
    );
    workspace.receive(Message::Result {
        generation: old_generation,
        id: api::JobId(10),
        result: Box::new(Err(crate::TransportError::Http { status: 500 })),
    });
    assert_eq!(workspace.artifact.as_ref(), Some(&first));
    assert!(matches!(
        workspace.task.error,
        Some(task::Error::Artifact(_))
    ));
}

#[test]
fn cancellation_waits_for_admission_and_worker_cleanup_then_can_load_a_partial_result() {
    let mut workspace = Workspace::default();
    let generation = start(&mut workspace);
    workspace.task.request_cancel();
    assert!(workspace.task.next(0.0).is_none());
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Box::new(Ok(fixture::snapshot(12, api::JobState::Queued, None))),
    });
    assert!(
        matches!(workspace.task.next(0.0), Some(Command::Cancel { id, generation: current }) if id == api::JobId(12) && current == generation)
    );
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Cancel,
        result: Box::new(Ok(fixture::snapshot(12, api::JobState::Cancelling, None))),
    });
    assert!(workspace.task.busy());
    assert!(
        matches!(workspace.task.next(1.0), Some(Command::Poll { id, generation: current }) if id == api::JobId(12) && current == generation)
    );
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Ok(fixture::snapshot(
            12,
            api::JobState::Cancelled,
            Some(fixture::version(1)),
        ))),
    });
    assert!(!workspace.task.busy());
    assert!(matches!(
        workspace.task.next(2.0),
        Some(Command::Result { .. })
    ));
    let mut partial = fixture::artifact(Strategy::Dispatcher);
    partial.completion = Completion::Cancelled;
    workspace.receive(Message::Result {
        generation,
        id: api::JobId(12),
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id: api::JobId(12),
                publication: fixture::version(1),
                artifact: partial,
            }),
        })),
    });
    assert!(
        workspace
            .accessible_status()
            .contains("Completion: Cancelled")
    );
    workspace.receive(Message::Status {
        generation,
        operation: JobOperation::Poll,
        result: Box::new(Ok(fixture::snapshot(12, api::JobState::Running, None))),
    });
    assert_eq!(workspace.task.label(), "Cancelled");
}

#[test]
fn stopped_attempt_without_a_validated_stage_keeps_the_previous_result_and_selection() {
    let mut workspace = Workspace::default();
    let previous = fixture::artifact(Strategy::Dispatcher);
    deliver(&mut workspace, previous.clone(), 14);
    let selected = workspace.state.selected.clone();
    let mut cancelled = fixture::artifact(Strategy::CodeDuplication);
    cancelled.completion = Completion::Cancelled;
    cancelled.stages.clear();
    cancelled.events.clear();
    cancelled.mappings.origins.clear();
    cancelled.current_stage = None;
    deliver(&mut workspace, cancelled, 15);
    assert_eq!(workspace.artifact.as_ref(), Some(&previous));
    assert_eq!(workspace.state.selected, selected);
    assert!(matches!(
        workspace.task.error,
        Some(task::Error::UnvalidatedAttempt {
            completion: Completion::Cancelled,
            ..
        })
    ));
    assert!(
        workspace
            .accessible_status()
            .contains("Previous artifact: true")
    );
    assert!(workspace.task.next(30.0).is_none());
}

#[test]
fn selection_navigation_and_copy_markers_use_real_qualified_occurrences() {
    let artifact = fixture::artifact(Strategy::CodeDuplication);
    model::check_artifact(&artifact).unwrap();
    let mut state = model::ViewState::default();
    let publication = model::Publication {
        job: api::JobId(21),
        version: fixture::version(1),
    };
    state.install(&artifact, publication);
    let original = EntityRef {
        stage: StageId(0),
        entity: EntityId(8),
    };
    let copy = EntityRef {
        stage: StageId(1),
        entity: EntityId(18),
    };
    assert!(state.navigate(&artifact, original));
    assert!(artifact.derived_from(original).contains(&copy));
    assert!(state.navigate(&artifact, copy));
    assert_eq!(state.current, Some(StageId(1)));
    assert_eq!(state.selected_in(&artifact, StageId(0)), None);
    assert_eq!(state.selected.as_ref().unwrap().artifact, artifact.id);
    assert_eq!(state.selected.as_ref().unwrap().publication, publication);
    assert!(artifact.descendants(3).contains(&copy));
    state.history(&artifact, false);
    assert_eq!(state.selected.as_ref().unwrap().at, original);
    assert_eq!(state.event, Some(0));
    state.history(&artifact, true);
    assert_eq!(state.selected.as_ref().unwrap().at, copy);
    assert_eq!(state.event, Some(1));
    let diff = Differences::new(&artifact, StageId(0), StageId(1));
    assert_eq!(diff.after[&EntityId(18)], [Marker::Copied].into());
    assert!(diff.after[&EntityId(30)].contains(&Marker::New));
    assert!(diff.before[&EntityId(2)].contains(&Marker::Removed));
    assert!(diff.after[&EntityId(9)].contains(&Marker::Redirected));
    for label in ["New", "Copied", "Removed", "Redirected"] {
        assert!(diff.summary().contains(label));
    }
    let second = fixture::artifact(Strategy::Dispatcher);
    state.install(
        &second,
        model::Publication {
            job: api::JobId(22),
            version: fixture::version(1),
        },
    );
    assert_eq!(state.selected_in(&artifact, StageId(1)), None);
    assert!(!state.can_back());
}

#[test]
fn failed_candidate_is_inspectable_while_current_remains_validated() {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    artifact.stages[1].validation.status = ValidationStatus::Failed;
    artifact.stages[1].ir.edges[0].to = EntityId(999);
    artifact.current_stage = Some(StageId(0));
    model::check_artifact(&artifact).unwrap();
    let mut workspace = Workspace::default();
    deliver(&mut workspace, artifact.clone(), 30);
    assert_eq!(workspace.state.current, Some(StageId(0)));
    assert_eq!(workspace.artifact.as_ref().unwrap().stages.len(), 2);
    artifact.mappings.origins[0].source.push(SourceSpan {
        pc: 100,
        end_pc: 101,
    });
    assert!(model::check_artifact(&artifact).is_err());
}

#[test]
fn ast_rendering_keeps_effects_and_both_structured_control_forms() {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    let stage = &mut artifact.stages[1];
    stage.ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(40),
            kind: StructuredKind::While {
                header: Box::new(StructuredNode {
                    id: EntityId(41),
                    kind: StructuredKind::Block { block: EntityId(1) },
                }),
                condition: Some(EntityId(3)),
                condition_nonzero: true,
                body: Box::new(StructuredNode {
                    id: EntityId(42),
                    kind: StructuredKind::If {
                        condition: EntityId(3),
                        then_branch: Box::new(StructuredNode {
                            id: EntityId(43),
                            kind: StructuredKind::Continue,
                        }),
                        else_branch: Box::new(StructuredNode {
                            id: EntityId(44),
                            kind: StructuredKind::Break,
                        }),
                    },
                }),
                exit_edge: Some(EntityId(9)),
            },
        },
    });
    let lines = super::structured::lines(stage);
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for fragment in [
        "while (true)",
        "if (v3",
        "evm.sstore",
        "continue;",
        "break;",
        "effect token",
    ] {
        assert!(text.contains(fragment), "{text}");
    }
    assert!(
        lines
            .iter()
            .any(|line| line.entity == EntityId(4) && line.text.contains("evm.sstore"))
    );
}

#[test]
fn structured_terminal_primitives_appear_once_after_prior_effects() {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    let stage = &mut artifact.stages[1];
    stage.ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(40),
            kind: StructuredKind::Sequence(vec![
                StructuredNode {
                    id: EntityId(41),
                    kind: StructuredKind::Block { block: EntityId(1) },
                },
                StructuredNode {
                    id: EntityId(42),
                    kind: StructuredKind::Exit { block: EntityId(1) },
                },
            ]),
        },
    });
    let nonterminal = stage.ir.blocks[0].operations.clone();
    for (kind, opcode, name, primitive, inputs) in [
        (ExitKind::Stop, 0x00, "STOP", "evm.stop(", vec![]),
        (
            ExitKind::Return,
            0xf3,
            "RETURN",
            "evm.return(",
            vec![EntityId(3), EntityId(3)],
        ),
        (
            ExitKind::Revert,
            0xfd,
            "REVERT",
            "evm.revert(",
            vec![EntityId(3), EntityId(3)],
        ),
        (
            ExitKind::SelfDestruct,
            0xff,
            "SELFDESTRUCT",
            "evm.selfdestruct(",
            vec![EntityId(3)],
        ),
        (ExitKind::EndOfCode, 0x00, "STOP", "evm.stop(", vec![]),
    ] {
        let block = &mut stage.ir.blocks[0];
        block.operations.clone_from(&nonterminal);
        if kind != ExitKind::EndOfCode {
            let mut terminal = nonterminal[0].clone();
            terminal.id = EntityId(50);
            terminal.opcode = opcode;
            terminal.name = name.into();
            terminal.inputs.clone_from(&inputs);
            terminal.effects = Effects {
                terminate: true,
                ..Effects::default()
            };
            block.operations.push(terminal);
        }
        block.terminator = Terminator::Exit {
            kind,
            values: inputs,
        };
        let text = super::structured::lines(stage)
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches(primitive).count(), 1, "{text}");
        assert!(text.find("evm.sstore(").unwrap() < text.find(primitive).unwrap());
        assert!(!text.contains("Exit {"), "{text}");
        if kind == ExitKind::EndOfCode {
            assert!(text.contains("end of code"));
        }
    }
    assert_eq!(
        super::structured::terminator(&Terminator::Pending),
        "partial_exit(\"recovery pending\");"
    );
    assert_eq!(
        super::structured::terminator(&Terminator::Trap {
            reason: "invalid opcode".into()
        }),
        "exception(\"invalid opcode\");"
    );
}
