//! A retained decoded source stage is disassembly, not recovered computation.
use evm_decompile::{
    Cancellation, Completion, Config, DiagnosticCode, Fork, Input, Phase, StageKind,
    TextExportStatus, ValueFact, decompile, decompile_with_progress, export_text,
    export_text_with_observer, render_text,
};

fn input() -> Input {
    Input::new(vec![0x60, 1, 0x60, 2, 0x01, 0x5f, 0x55, 0x00], Fork::Osaka)
}

fn source_config() -> Config {
    let mut config = Config::default();
    config.limits.max_iterations = 0;
    config
}

#[test]
fn decoded_source_export_shows_original_instructions_without_fake_ssa_or_effects() {
    let artifact = decompile(&input(), &source_config());
    let retained = artifact.clone();
    assert_eq!(artifact.completion, Completion::Limited);
    let source = artifact.current().unwrap();
    assert_eq!(source.kind, StageKind::Decoded);
    assert!(
        source
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .all(|op| op.inputs.is_empty())
    );
    assert!(source.ir.values.iter().any(|v| v.fact == ValueFact::Bottom));
    let exported = export_text(&artifact, &Cancellation::default());
    assert_eq!(exported.status, TextExportStatus::Complete);
    assert!(exported.work > 0);
    let text = exported.text;
    assert!(text.contains("decoded source only"));
    assert!(text.contains("stack/value recovery unavailable"));
    assert!(text.contains("0x0000: PUSH1 0x01"));
    assert!(text.contains("0x0002: PUSH1 0x02"));
    assert!(text.contains("0x0004: ADD"));
    assert!(text.contains("0x0005: PUSH0"));
    assert!(text.contains("0x0006: SSTORE"));
    assert!(text.contains("0x0007: STOP"));
    assert_eq!(text.matches("; unrecovered;").count(), 6);
    assert!(text.contains("Limit") && text.contains("block transfer iteration limit exhausted"));
    for invented in [" = ", "add(", "sstore(", "stop(", "partial_recovery("] {
        assert!(!text.contains(invented), "{text}");
    }
    assert_eq!(render_text(&artifact), text);
    assert_eq!(
        artifact, retained,
        "export must preserve the last validated artifact"
    );
    eprintln!("{text}");
}

#[test]
fn decoded_source_export_obeys_explicit_work_and_cancellation() {
    let artifact = decompile(&input(), &source_config());
    let cancel = Cancellation::new();
    let limited = export_text_with_observer(&artifact, &cancel, 1, |_| {});
    assert_eq!(limited.status, TextExportStatus::Limited);
    assert!(limited.work <= 1);
    assert!(limited.text.contains("work limit exhausted"));
    let mut checks = 0;
    let cancelled = export_text_with_observer(&artifact, &cancel, usize::MAX, |_| {
        checks += 1;
        // Diagnostic and stage reservations precede the first two source ops.
        if checks == 4 {
            cancel.cancel();
        }
    });
    assert_eq!(cancelled.status, TextExportStatus::Cancelled);
    assert!(cancelled.work > 0);
    assert!(cancelled.text.contains("0x0000: PUSH1"));
    assert!(!cancelled.text.contains("0x0007: STOP"));
    assert!(cancelled.text.contains("export cancelled"));
}

#[test]
fn decoded_source_export_preserves_actual_truncated_push_bytes() {
    let artifact = decompile(&Input::new(vec![0x61, 0xab], Fork::Osaka), &source_config());
    let text = render_text(&artifact);
    assert!(text.contains("0x0000: PUSH2 0xab // bytes 61ab; unrecovered;"));
    assert!(!text.contains(" = "));
}

#[test]
fn no_stage_export_includes_decode_and_limit_diagnostics() {
    let failed = decompile(
        &Input::new(vec![0xef, 0x00], Fork::Osaka),
        &Config::default(),
    );
    assert_eq!(failed.completion, Completion::Failed);
    assert!(failed.current().is_none());
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::Decode)
    );
    let exported = export_text(&failed, &Cancellation::default());
    assert_eq!(exported.status, TextExportStatus::Complete);
    assert!(exported.work > 0);
    assert!(exported.text.contains("no validated stage"));
    assert!(exported.text.contains("Decode"));
    assert!(exported.text.contains(&failed.diagnostics[0].message));
    let mut config = Config::default();
    config.limits.max_bytecode_bytes = 0;
    let limited = decompile(&input(), &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert!(limited.current().is_none());
    let exported = export_text(&limited, &Cancellation::default());
    assert!(exported.text.contains("max_bytecode_bytes"));
    assert_eq!(exported.status, TextExportStatus::Complete);
    let cancel = Cancellation::new();
    cancel.cancel();
    assert_eq!(
        export_text(&failed, &cancel).status,
        TextExportStatus::Cancelled
    );
}

#[test]
fn recovered_baseline_and_partial_ssa_keep_real_operands() {
    let cancel = Cancellation::new();
    let artifact = decompile_with_progress(&input(), &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Simplify {
            cancel.cancel();
        }
    });
    assert_eq!(artifact.completion, Completion::Cancelled);
    assert_eq!(artifact.current().unwrap().kind, StageKind::Baseline);
    let exported = export_text(&artifact, &Cancellation::default());
    assert_eq!(exported.status, TextExportStatus::Complete);
    assert!(exported.text.contains("sstore(0x0, v"));
    assert!(!exported.text.contains("unrecovered"));
    let partial = decompile(
        &Input::new(vec![0x5f, 0x35, 0x56], Fork::Osaka),
        &Config::default(),
    );
    assert_eq!(partial.completion, Completion::Partial);
    let text = render_text(&partial);
    assert!(text.contains("calldataload(0x0)"));
    assert!(text.contains("partial_indirect_exit"));
    assert!(!text.contains("decoded source only"));
}

#[test]
fn explicit_export_cancellation_is_independent_of_cancelled_ambient_scope() {
    let ambient = Cancellation::new();
    ambient.cancel();
    let own = Cancellation::new();
    analysis_control::with_cancellation(&ambient, || {
        let artifact = evm_decompile::decompile_with_cancel(
            &Input::new(vec![0x00], Fork::Osaka),
            &Config::default(),
            &own,
        );
        assert_eq!(artifact.completion, Completion::Complete);
        let exported = export_text_with_observer(&artifact, &own, usize::MAX, |_| {});
        assert_eq!(exported.status, TextExportStatus::Complete);
        assert!(exported.work > 0);
        assert!(!own.is_cancelled());
        assert!(analysis_control::current_cancellation().is_cancelled());
        own.cancel();
        assert_eq!(
            export_text(&artifact, &own).status,
            TextExportStatus::Cancelled
        );
    });
}
