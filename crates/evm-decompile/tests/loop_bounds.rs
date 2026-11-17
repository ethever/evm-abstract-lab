//! A must-bound component must not undo the numeric domain's acceleration.
use evm_decompile::{
    Artifact, Cancellation, Completion, Config, ExecutionLimits, Fork, Input, MemoryHost, Phase,
    StageKind, U256, ValidationStatus, decompile, decompile_with_progress, execute_bytecode,
    execute_ir, execute_structured,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
use assembly::Asm;

#[test]
fn a_large_descending_counter_still_converges_under_finite_work() {
    let mut a = Asm::default();
    a.word(U256::MAX)
        .dest("head")
        .op(0x80)
        .op(0x15)
        .target("done")
        .op(0x57)
        .number(1)
        .op(0x90)
        .op(0x03)
        .target("head")
        .op(0x56)
        .dest("done")
        .op(0x50)
        .op(0x00);
    let input = a.finish();
    let mut config = Config::default();
    config.limits.max_constant_set = 2;
    config.limits.max_work = 30_000_000;
    let artifact = decompile(&input, &config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    assert!(artifact.statistics.work <= config.limits.max_work);
}

fn unsafe_loop(no_progress: bool) -> Input {
    let mut a = Asm::default();
    a.target("first")
        .number(0x80)
        .op(0x52)
        .target("second")
        .number(0xa0)
        .op(0x52)
        .number(0)
        .op(0x35)
        .number(255)
        .op(0x16)
        .number(0)
        .number(0x480)
        .dest("head")
        .op(0x82)
        .op(0x82)
        .op(0x10)
        .op(0x15)
        .target("done")
        .op(0x57)
        .number(7)
        .op(0x81)
        .op(0x52)
        .number(32)
        .op(0x35)
        .number(1)
        .op(0x16)
        .number(5)
        .op(0x1b)
        .number(0x80)
        .op(0x01)
        .op(0x51)
        .op(0x56);
    for label in ["first", "second"] {
        a.dest(label);
        if no_progress {
            // A bounded end alone cannot justify a bound on an unrelated
            // pointer when the cursor never advances.
            a.number(256).op(0x01);
        } else {
            a.op(0x90)
                .number(1)
                .op(0x01)
                .op(0x90)
                .op(0x50)
                .number(64)
                .op(0x35);
        }
        a.target("head").op(0x56);
    }
    a.dest("done").op(0x00);
    a.finish()
}

#[test]
fn missing_progress_or_an_opaque_pointer_assignment_cannot_invent_a_loop_bound() {
    for no_progress in [true, false] {
        let input = unsafe_loop(no_progress);
        let mut config = Config::default();
        config.limits.max_constant_set = 2;
        config.limits.max_work = 100_000_000;
        let artifact = decompile(&input, &config);
        assert_eq!(
            artifact.completion,
            Completion::Partial,
            "{:?}",
            artifact.diagnostics
        );
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.validation.status == ValidationStatus::Passed)
        );
        assert!(
            artifact
                .current()
                .unwrap()
                .ir
                .blocks
                .iter()
                .any(|b| b.terminator.is_partial())
        );
    }
}

fn guarded_input(skip: bool, different_input: bool) -> Input {
    let mut a = Asm::default();
    if skip {
        a.number(32).op(0x35).target("join").op(0x57);
    }
    // Both compared operands disappear before the later read. A temporary
    // stack binding cannot carry this immutable-input bound across the guard.
    a.number(0)
        .op(0x35)
        .number(256)
        .op(0x90)
        .op(0x10)
        .target("join")
        .op(0x57)
        .number(0)
        .number(0)
        .op(0xfd)
        .dest("join")
        .number(if different_input { 64 } else { 0 })
        .op(0x35)
        .number(256)
        .op(0x90)
        .op(0x10)
        .number(0)
        .op(0x55)
        .op(0x00);
    a.finish()
}

fn concrete(input: &Input, artifact: &Artifact, words: &[u64]) -> MemoryHost {
    let initial = MemoryHost::new(
        input,
        words
            .iter()
            .flat_map(|word| U256::from(*word).to_be_bytes::<32>())
            .collect(),
    );
    let limits = ExecutionLimits::default();
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut host = initial.clone();
        let execution = execute_ir(stage, &mut host, &limits).unwrap();
        assert_eq!(execution.outcome, raw.outcome);
        assert_eq!(execution.observations(), raw.observations());
        assert_eq!(host, raw_host);
        if stage.ir.structured.is_some() {
            let mut host = initial.clone();
            let execution = execute_structured(stage, &mut host, &limits).unwrap();
            assert_eq!(execution.outcome, raw.outcome);
            assert_eq!(execution.observations(), raw.observations());
            assert_eq!(host, raw_host);
        }
    }
    raw_host
}

#[test]
fn popped_guard_operands_still_bound_the_same_immutable_input() {
    let input = guarded_input(false, false);
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Complete);
    let baseline = artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .unwrap();
    let checks: Vec<_> = baseline
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter(|o| o.opcode == 0x10)
        .collect();
    assert_eq!(checks.len(), 2);
    assert_eq!(
        baseline
            .ir
            .value(checks[1].outputs[0])
            .unwrap()
            .fact
            .singleton(),
        Some(U256::from(1))
    );
    for value in [0, 255, 256] {
        let _ = concrete(&input, &artifact, &[value]);
    }
}

#[test]
fn an_unguarded_join_or_a_different_input_cannot_borrow_the_cached_guard() {
    for (skip, different) in [(true, false), (false, true)] {
        let input = guarded_input(skip, different);
        let artifact = decompile(&input, &Config::default());
        assert_eq!(artifact.completion, Completion::Complete);
        let baseline = artifact
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Baseline)
            .unwrap();
        let check = baseline
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .rfind(|o| o.opcode == 0x10)
            .unwrap();
        assert!(
            baseline
                .ir
                .value(check.outputs[0])
                .unwrap()
                .fact
                .singleton()
                .is_none()
        );
        let observed = concrete(
            &input,
            &artifact,
            if skip { &[256, 1, 0] } else { &[0, 0, 256] },
        );
        assert_eq!(observed.storage[&U256::ZERO], U256::ZERO);
        let _ = concrete(&input, &artifact, &[255, 0, 255]);
    }
}

#[test]
fn interrupted_bound_discovery_never_publishes_a_partial_recovery() {
    let input = unsafe_loop(true);
    let mut config = Config::default();
    config.limits.max_constant_set = 2;
    config.limits.max_work = 100_000_000;
    let mut checkpoints = Vec::new();
    let complete = decompile_with_progress(&input, &config, &Cancellation::new(), |p| {
        if p.phase == Phase::Recovery {
            checkpoints.push(p.work);
        }
    });
    assert_eq!(complete.completion, Completion::Partial);
    assert!(checkpoints.len() > 2);
    let cutoff = checkpoints[1];
    let cancel = Cancellation::new();
    let mut cancellation_config = config.clone();
    cancellation_config.limits.max_work = u64::MAX;
    let cancelled = decompile_with_progress(&input, &cancellation_config, &cancel, |p| {
        if p.phase == Phase::Recovery && p.work >= cutoff {
            cancel.cancel();
        }
    });
    assert_eq!(cancelled.completion, Completion::Cancelled);
    config.limits.max_work = cutoff;
    let limited = decompile(&input, &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert!(limited.statistics.work <= cutoff);
    for attempt in [cancelled, limited] {
        assert_eq!(attempt.current().unwrap().kind, StageKind::Decoded);
        assert!(
            attempt
                .stages
                .iter()
                .all(|s| s.validation.status == ValidationStatus::Passed)
        );
        assert!(attempt.events.iter().all(|e| e.rule != "local_recovery"));
    }
}
