//! Loop memory bounds must survive mixed progress without hiding real aliases.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, Fork, Input,
    MemoryHost, StageKind, U256, ValidationStatus, decompile, execute_bytecode, execute_ir,
    execute_structured,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
use assembly::Asm;

fn loop_input(overwrite_table: bool, bounded_end: bool) -> Input {
    let mut a = Asm::default();
    a.target("short").number(0x80).op(0x52);
    a.target("long").number(0xa0).op(0x52);
    a.number(0).op(0x35);
    if bounded_end {
        a.number(0xff).op(0x16);
    }
    a.number(0).number(0x400); // end, cursor, output pointer
    a.dest("loop");
    a.op(0x82)
        .op(0x82)
        .op(0x10)
        .op(0x15)
        .target("done")
        .op(0x57);
    a.number(7).op(0x81).op(0x52); // write the output record header
    if overwrite_table {
        a.number(64).op(0x35).number(0x80).op(0x52);
    }
    a.number(32).op(0x35).number(1).op(0x16);
    a.number(5).op(0x1b).number(0x80).op(0x01).op(0x51).op(0x56);
    for (label, step) in [("short", 1), ("long", 185)] {
        a.dest(label).op(0x90).number(step).op(0x01).op(0x90);
        a.number(0x100).op(0x01).target("loop").op(0x56);
    }
    a.dest("done").op(0x80).number(0).op(0x55);
    a.op(0x81).number(1).op(0x55).op(0x00);
    a.finish()
}

fn analyze(input: &Input) -> Artifact {
    let mut config = Config::default();
    // Force the address component to stop enumerating long before loop exit.
    config.limits.max_constant_set = 2;
    config.limits.max_work = 100_000_000;
    let artifact = decompile(input, &config);
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| { stage.validation.status == ValidationStatus::Passed }),
        "{:?}",
        artifact.diagnostics
    );
    artifact
}

#[test]
fn bounded_mixed_progress_keeps_the_table_after_finite_addresses_overflow() {
    let input = loop_input(false, true);
    let artifact = analyze(&input);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    for length in [0_u64, 1, 17, 65, 255] {
        for tag in [0_u64, 1] {
            let data = [length, tag, 0]
                .into_iter()
                .flat_map(|n| U256::from(n).to_be_bytes::<32>())
                .collect();
            let initial = MemoryHost::new(&input, data);
            let limits = ExecutionLimits::default();
            let mut raw_host = initial.clone();
            let raw = execute_bytecode(&input, &mut raw_host, &limits).unwrap();
            assert!(matches!(
                raw.outcome,
                ExecutionOutcome::Exit {
                    kind: ExitKind::Stop,
                    ..
                }
            ));
            let step = if tag == 0 { 1 } else { 185 };
            let rounds = length.div_ceil(step);
            assert_eq!(
                raw_host.storage[&U256::ZERO],
                U256::from(0x400 + 0x100 * rounds)
            );
            assert_eq!(raw_host.storage[&U256::from(1)], U256::from(step * rounds));
            for stage in artifact
                .stages
                .iter()
                .filter(|stage| stage.kind != StageKind::Decoded)
            {
                let mut host = initial.clone();
                let result = execute_ir(stage, &mut host, &limits).unwrap();
                assert_eq!(result.observations(), raw.observations());
                assert_eq!(host.memory, raw_host.memory);
                assert_eq!(host.storage, raw_host.storage);
                if stage.ir.structured.is_some() {
                    let mut host = initial.clone();
                    let result = execute_structured(stage, &mut host, &limits).unwrap();
                    assert_eq!(result.observations(), raw.observations());
                    assert_eq!(host.memory, raw_host.memory);
                    assert_eq!(host.storage, raw_host.storage);
                }
            }
        }
    }
}

#[test]
fn actual_table_alias_and_missing_counter_bound_keep_unknown_exits() {
    for input in [loop_input(true, true), loop_input(false, false)] {
        let artifact = analyze(&input);
        assert_eq!(artifact.completion, Completion::Partial);
        assert!(
            artifact
                .current()
                .unwrap()
                .ir
                .blocks
                .iter()
                .any(|block| block.terminator.is_partial())
        );
    }
}
