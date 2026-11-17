//! Immutable input bounds must survive rereads only when every incoming path proves them.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, Fork, Input,
    MemoryHost, StageKind, U256, ValidationStatus, ValueFact, decompile, execute_bytecode,
    execute_ir,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
use assembly::Asm;

fn guarded_reread(both_guarded: bool, same_input: bool, threshold: u64) -> Input {
    let mut a = Asm::default();
    a.number(32).op(0x35).target("right").op(0x57);
    // Reject x > 255, consuming both comparison operands before the merge.
    a.number(255)
        .number(0)
        .op(0x35)
        .op(0x11)
        .target("reject")
        .op(0x57);
    a.target("join").op(0x56);
    a.dest("right");
    if both_guarded {
        a.number(1023)
            .number(0)
            .op(0x35)
            .op(0x11)
            .target("reject")
            .op(0x57);
    }
    a.dest("join");
    a.number(threshold)
        .number(if same_input { 0 } else { 64 })
        .op(0x35)
        .op(0x11);
    a.target("large").op(0x57);
    a.number(1).number(0).op(0x55).op(0x00);
    a.dest("large").number(2).number(0).op(0x55).op(0x00);
    a.dest("reject").number(0).number(0).op(0xfd);
    a.finish()
}

fn analyze(input: &Input) -> Artifact {
    let mut config = Config::default();
    config.limits.max_work = 30_000_000;
    let artifact = decompile(input, &config);
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
    artifact
}

#[test]
fn joined_guards_use_the_hull_of_every_path_and_preserve_repeated_input_observations() {
    let input = guarded_reread(true, true, 1023);
    let artifact = analyze(&input);
    let program =
        evm_bytecode::Program::decode_with_fork(&input.runtime_bytecode, input.fork).unwrap();
    let final_compare = program
        .blocks()
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|instruction| instruction.opcode == 0x11)
        .map(|instruction| instruction.pc)
        .max()
        .unwrap();
    let baseline = &artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .unwrap()
        .ir;
    let comparison = baseline
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| op.source.is_some_and(|span| span.pc == final_compare))
        .unwrap();
    assert_eq!(
        baseline.value(comparison.outputs[0]).unwrap().fact,
        ValueFact::Constants(vec![U256::ZERO]),
        "the reread must retain the joined input bound"
    );
    for (x, route, rejected) in [
        (7_u64, 0_u64, false),
        (1000, 1, false),
        (1000, 0, true),
        (1024, 1, true),
    ] {
        let calldata = [x, route, 9999]
            .into_iter()
            .flat_map(|n| U256::from(n).to_be_bytes::<32>())
            .collect();
        let initial = MemoryHost::new(&input, calldata);
        let mut raw_host = initial.clone();
        let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
        assert!(
            matches!(raw.outcome,ExecutionOutcome::Exit{kind,..} if kind==if rejected {ExitKind::Revert} else {ExitKind::Stop})
        );
        if !rejected {
            assert_eq!(raw_host.storage[&U256::ZERO], U256::from(1));
        }
        for stage in artifact
            .stages
            .iter()
            .filter(|stage| stage.kind != StageKind::Decoded)
        {
            let mut host = initial.clone();
            let observed = execute_ir(stage, &mut host, &ExecutionLimits::default()).unwrap();
            assert_eq!(observed.observations(), raw.observations());
            assert_eq!(host.storage, raw_host.storage);
        }
    }
}

#[test]
fn an_unguarded_predecessor_or_a_different_input_cannot_borrow_the_cached_bound() {
    for (both_guarded, same_input) in [(false, true), (true, false)] {
        let input = guarded_reread(both_guarded, same_input, 1023);
        let artifact = analyze(&input);
        let x = if both_guarded { 7 } else { 5000 };
        let calldata = [x, 1, 5000]
            .into_iter()
            .flat_map(|n| U256::from(n).to_be_bytes::<32>())
            .collect();
        let initial = MemoryHost::new(&input, calldata);
        let mut raw_host = initial.clone();
        let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
        assert_eq!(raw_host.storage[&U256::ZERO], U256::from(2));
        for stage in artifact
            .stages
            .iter()
            .filter(|stage| stage.kind != StageKind::Decoded)
        {
            let mut host = initial.clone();
            let observed = execute_ir(stage, &mut host, &ExecutionLimits::default()).unwrap();
            assert_eq!(observed.observations(), raw.observations());
            assert_eq!(host.storage, raw_host.storage);
        }
    }
}

#[test]
fn different_guard_bounds_are_united_instead_of_intersected_at_the_join() {
    let input = guarded_reread(true, true, 255);
    let artifact = analyze(&input);
    for (x, route, marker) in [(200_u64, 0_u64, 1_u64), (1000, 1, 2)] {
        let calldata = [x, route, 0]
            .into_iter()
            .flat_map(|n| U256::from(n).to_be_bytes::<32>())
            .collect();
        let initial = MemoryHost::new(&input, calldata);
        let mut raw_host = initial.clone();
        let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
        assert_eq!(raw_host.storage[&U256::ZERO], U256::from(marker));
        for stage in artifact
            .stages
            .iter()
            .filter(|s| s.kind != StageKind::Decoded)
        {
            let mut host = initial.clone();
            let actual = execute_ir(stage, &mut host, &ExecutionLimits::default()).unwrap();
            assert_eq!(actual.observations(), raw.observations());
            assert_eq!(host.storage, raw_host.storage);
        }
    }
}
