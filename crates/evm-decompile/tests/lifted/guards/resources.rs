use super::*;
use evm_decompile::{Cancellation, Phase, decompile_with_progress};

#[test]
fn guard_proof_metadata_work_history_and_cancellation_are_atomic() {
    let input = decoder(false);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel();
        }
    });
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let complete = checked(&input);
    assert!(guards(&complete) >= 4);
    assert!(complete.statistics.history_entities > before.statistics.history_entities);
    assert!(complete.statistics.work > before.statistics.work);
    let retained = |artifact: &Artifact| {
        validated(artifact);
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
        let artifact = decompile(&input, &config);
        assert_eq!(artifact.completion, Completion::Limited);
        retained(&artifact);
        assert!(artifact.statistics.work <= config.limits.max_work);
        assert!(artifact.statistics.history_entities <= config.limits.max_history_entities);
    }
    for phase in [Phase::Validate, Phase::Provenance] {
        let cancel = Cancellation::new();
        let mut lifting = false;
        let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
            lifting |= progress.phase == Phase::Lift;
            if lifting && progress.phase == phase {
                cancel.cancel();
            }
        });
        assert_eq!(artifact.completion, Completion::Cancelled);
        retained(&artifact);
    }
}
