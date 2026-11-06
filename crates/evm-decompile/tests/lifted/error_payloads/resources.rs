use super::*;
use evm_decompile::{Cancellation, Completion, Phase, decompile_with_progress};

#[test]
fn nonempty_proofs_work_history_and_cancellation_publish_atomically() {
    let input = fixtures::guarded(TRANSFER, false, None);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel()
        }
    });
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let complete = checked(&input);
    assert_message(&complete, TRANSFER);
    assert!(complete.statistics.history_entities > before.statistics.history_entities);
    let retained = |artifact: &Artifact| {
        super::super::support::validated(artifact);
        assert_eq!(artifact.stages, before.stages);
        assert_eq!(artifact.events, before.events);
        assert_eq!(artifact.mappings, before.mappings);
        assert!(artifact.current().unwrap().ir.lifted.is_none());
    };
    for history in [false, true] {
        let mut config = Config::default();
        if history {
            config.limits.max_history_entities = complete.statistics.history_entities - 1;
        } else {
            config.limits.max_work = before.statistics.work;
        }
        let limited = decompile(&input, &config);
        assert_eq!(limited.completion, Completion::Limited);
        retained(&limited);
        assert!(limited.statistics.work <= config.limits.max_work);
        assert!(limited.statistics.history_entities <= config.limits.max_history_entities);
    }
    for phase in [Phase::Validate, Phase::Provenance] {
        let cancellation = Cancellation::new();
        let mut lifting = false;
        let cancelled =
            decompile_with_progress(&input, &Config::default(), &cancellation, |progress| {
                lifting |= progress.phase == Phase::Lift;
                if lifting && progress.phase == phase {
                    cancellation.cancel()
                }
            });
        assert_eq!(cancelled.completion, Completion::Cancelled);
        retained(&cancelled);
    }
    let (stage, program) = lifted(&complete);
    let structured = complete
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Structured)
        .unwrap();
    let diff = complete.diff(structured.id, stage.id).unwrap();
    for id in program
        .payloads
        .iter()
        .map(|p| p.id)
        .chain(program.reverts.iter().map(|p| p.id))
        .chain(program.constructions.iter().map(|p| p.id))
    {
        let origin = complete
            .origin(evm_decompile::EntityRef {
                stage: stage.id,
                entity: id,
            })
            .expect("payload and proof identities have inspectable origins");
        assert!(!origin.parents.is_empty());
        assert!(!origin.source.is_empty());
        assert!(
            diff.added.contains(&id),
            "new proof identity is absent from the stage diff"
        );
    }
    for construction in &program.constructions {
        let origin = complete
            .origin(evm_decompile::EntityRef {
                stage: stage.id,
                entity: construction.id,
            })
            .unwrap();
        let body = program.block(construction.block).unwrap();
        for statement in &body.statements[construction.start..construction.end] {
            assert!(
                origin
                    .parents
                    .iter()
                    .any(|parent| parent.entity == statement.operation),
                "absorbed operation must remain navigable through construction provenance"
            );
        }
    }
    assert_eq!(
        complete,
        serde_json::from_slice(&serde_json::to_vec(&complete).unwrap()).unwrap()
    );
}

#[test]
fn escaped_nonempty_export_refusal_is_atomic_at_the_shared_formatter_boundary() {
    let input = fixtures::guarded(
        "quote\" slash\\ newline\n nul\0 中文".as_bytes(),
        false,
        None,
    );
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let index = program.render_index(&stage.ir);
    let block = program.constructions[0].block;
    let mut full_work = 0_usize;
    let expected = index
        .render_revert(block, |work| {
            full_work += work;
            true
        })
        .unwrap()
        .unwrap();
    assert!(full_work > 0);
    for initial in [0, full_work - 1, full_work] {
        let mut remaining = initial;
        let rendered = index.render_revert(block, |work| {
            if work > remaining {
                false
            } else {
                remaining -= work;
                true
            }
        });
        if initial < full_work {
            assert_eq!(rendered, Err(evm_decompile::LiftedRenderError::Limit));
        } else {
            assert_eq!(rendered.unwrap(), Some(expected.clone()));
        }
    }
    let control = program.controls.iter().find(|c| c.guard.is_some()).unwrap();
    assert_eq!(
        index.render_guard(control, |_| false),
        Err(evm_decompile::LiftedRenderError::Limit)
    );
}
