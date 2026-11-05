//! Core-owned private tests for the wide-model/native execution boundary.

use super::{NativeLimitError, constant_capacity, count, native, work_budget};

#[test]
fn native_32_boundary_rejects_large_work_without_clamping() {
    assert_eq!(native::<u32>(4_294_967_295, "max_work"), Ok(u32::MAX));
    for value in [
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        assert_eq!(
            native::<u32>(value, "max_work"),
            Err(NativeLimitError::Unrepresentable {
                field: "max_work",
                value,
                bits: 32,
            })
        );
    }
}

#[test]
fn zero_work_and_nonzero_constant_capacity_keep_their_meanings() {
    let mut budget = work_budget(0).unwrap();
    assert!(budget.charge(0));
    assert!(!budget.charge(1));
    assert_eq!(budget.used(), 0);
    assert_eq!(constant_capacity(0), Err(NativeLimitError::ZeroConstantSet));
    assert_eq!(constant_capacity(8).unwrap().get(), 8);
    assert_eq!(count(0), 0);
    assert_eq!(count(usize::MAX), u64::try_from(usize::MAX).unwrap());
}

#[cfg(target_pointer_width = "64")]
#[test]
fn native_64_accepts_maximum_without_preallocating() {
    let budget = work_budget(u64::MAX).unwrap();
    assert_eq!(count(budget.limit()), u64::MAX);
    assert_eq!(budget.used(), 0);
    assert_eq!(count(constant_capacity(u64::MAX).unwrap().get()), u64::MAX);
}

#[cfg(target_pointer_width = "32")]
#[test]
fn unsupported_config_is_frozen_and_refused_before_work() {
    use crate::{Cancellation, Completion, Config, DiagnosticCode, Fork, Input, Phase};

    let input = Input::new(vec![0x00], Fork::Osaka);
    let config = Config::default();
    let mut progress = Vec::new();
    let artifact =
        crate::decompile_with_progress(&input, &config, &Cancellation::default(), |value| {
            progress.push(value)
        });
    assert_eq!(artifact.config, config);
    assert_eq!(artifact.completion, Completion::Failed);
    assert_eq!(artifact.diagnostics.len(), 1);
    assert_eq!(artifact.diagnostics[0].code, DiagnosticCode::Limit);
    assert!(artifact.diagnostics[0].message.contains("max_work"));
    assert_eq!(artifact.statistics.work, 0);
    assert_eq!(artifact.statistics.evaluations, 0);
    assert!(artifact.stages.is_empty());
    assert_eq!(progress.first().unwrap().phase, Phase::Decode);
    assert_eq!(progress.last().unwrap().phase, Phase::Complete);
    assert!(progress.iter().all(|value| value.work == 0));

    let export = crate::export_text(&artifact, &Cancellation::default());
    assert_eq!(export.status, crate::TextExportStatus::Limited);
    assert_eq!(export.work, 0);
    assert!(export.text.contains("max_work"));
}
