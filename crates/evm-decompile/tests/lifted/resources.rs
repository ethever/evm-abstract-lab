use super::oracle::certify;
use super::support::{Asm, calldata, checked, comparison_branch, validated};
use evm_decompile::{
    Cancellation, Completion, Config, Input, MemoryHost, Phase, StageKind, TextExportStatus, U256,
    decompile, decompile_with_progress, export_text, export_text_with_observer,
};

fn shared_sum(depth: usize) -> Input {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35);
    for _ in 0..depth {
        asm.op(0x80).op(0x01);
    }
    asm.op(0x5f).op(0x55).op(0x00);
    asm.finish()
}

#[test]
fn truncated_shared_dag_export_never_claims_complete_semantic_source() {
    let input = shared_sum(32);
    let artifact = checked(&input);
    // Only 32 pure operations, but inlining their shared operands expands to
    // billions of leaves. The flat semantic DAG and its evaluation remain valid.
    let (_, host) = certify(
        &input,
        &artifact,
        MemoryHost::new(&input, calldata(&[U256::from(3)])),
    );
    assert_eq!(host.storage[&U256::ZERO], U256::from(3) << 32);
    let export = export_text(&artifact, &Cancellation::new());
    assert_eq!(
        export.status,
        TextExportStatus::Limited,
        "expanded-expression refusal reported Complete: {}",
        export.text
    );
    assert!(
        export
            .text
            .contains("export cancelled or work limit exhausted")
    );
    assert_eq!(artifact.completion, Completion::Complete);
}

#[test]
fn export_expression_expansion_is_metered_and_cancellable_inside_the_statement() {
    let input = shared_sum(10);
    let artifact = checked(&input);
    let mut observations = Vec::new();
    let complete = export_text_with_observer(&artifact, &Cancellation::new(), 1_000_000, |work| {
        observations.push(work)
    });
    assert_eq!(complete.status, TextExportStatus::Complete);
    assert!(complete.text.contains("sstore("));
    assert!(!complete.text.contains("expression_limit"));
    assert!(
        complete.work >= 1024,
        "the 1,024 expanded leaves were not charged"
    );
    assert_eq!(observations.last(), Some(&complete.work));
    assert!(observations.windows(2).all(|pair| pair[0] <= pair[1]));
    let allowance = complete.work / 2;
    let limited = export_text_with_observer(&artifact, &Cancellation::new(), allowance, |_| {});
    assert_eq!(limited.status, TextExportStatus::Limited);
    assert!(limited.work <= allowance);
    assert!(
        limited.text.contains("calldataload("),
        "refusal must happen after input production"
    );
    assert!(
        !limited.text.contains("sstore("),
        "partially rendered invocation must not be published as complete"
    );
    let cancel = Cancellation::new();
    let cancelled = export_text_with_observer(&artifact, &cancel, 1_000_000, |work| {
        if work >= allowance {
            cancel.cancel();
        }
    });
    assert!(cancel.is_cancelled());
    assert_eq!(cancelled.status, TextExportStatus::Cancelled);
    assert!(cancelled.work < complete.work);
    assert!(cancelled.text.contains("calldataload("));
    assert!(!cancelled.text.contains("sstore("));
    assert_eq!(artifact.completion, Completion::Complete);
}

#[test]
fn long_boolean_history_can_refuse_or_cancel_after_lifting_has_done_work() {
    let input = comparison_branch(0x10, 512);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel();
        }
    });
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let mut config = Config::default();
    config.limits.max_work = before.statistics.work + 131_072;
    let refused = decompile(&input, &config);
    assert_eq!(refused.completion, Completion::Limited);
    assert!(refused.statistics.work <= config.limits.max_work);
    assert_eq!(refused.stages, before.stages);
    assert_eq!(refused.events, before.events);
    assert_eq!(refused.mappings, before.mappings);
    let cancel = Cancellation::new();
    let mut work_at_lift = None;
    let mut cancelled_after_work = false;
    let cancelled = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Lift {
            let start = *work_at_lift.get_or_insert(progress.work);
            if progress.work >= start + 65_536 {
                cancelled_after_work = true;
                cancel.cancel();
            }
        }
    });
    assert!(
        cancelled_after_work,
        "sustained lifting work did not expose a cooperative cancellation checkpoint"
    );
    assert_eq!(cancelled.completion, Completion::Cancelled);
    assert_eq!(cancelled.stages, before.stages);
    assert_eq!(cancelled.events, before.events);
    assert_eq!(cancelled.mappings, before.mappings);
    validated(&cancelled);
    validated(&refused);
}
