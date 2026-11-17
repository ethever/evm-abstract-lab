//! May-writes must preserve disjoint cells without strongly writing every choice.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, Fork, Input, MemoryHost, StageKind, U256,
    ValidationStatus, ValueFact, decompile, execute_bytecode, execute_ir, execute_structured,
};
#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
use assembly::Asm;

fn checked(input: &Input) -> Artifact {
    let artifact = decompile(input, &Config::default());
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
            .all(|s| s.validation.status == ValidationStatus::Passed)
    );
    artifact
}
fn loads(artifact: &Artifact) -> Vec<&ValueFact> {
    let ir = &artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .unwrap()
        .ir;
    ir.blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter(|o| o.opcode == 0x51)
        .map(|o| &ir.value(o.outputs[0]).unwrap().fact)
        .collect()
}
fn oracle(input: &Input, artifact: &Artifact, words: &[U256]) {
    let state = MemoryHost::new(
        input,
        words.iter().flat_map(|w| w.to_be_bytes::<32>()).collect(),
    );
    let limits = ExecutionLimits::default();
    let mut raw_host = state.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits);
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut host = state.clone();
        let actual = execute_ir(stage, &mut host, &limits);
        assert_eq!(
            actual.as_ref().map(|e| &e.outcome),
            raw.as_ref().map(|e| &e.outcome)
        );
        assert_eq!(host, raw_host);
        if stage.ir.structured.is_some() {
            let mut host = state.clone();
            let actual = execute_structured(stage, &mut host, &limits);
            assert_eq!(
                actual.as_ref().map(|e| &e.outcome),
                raw.as_ref().map(|e| &e.outcome)
            );
            assert_eq!(host, raw_host);
        }
    }
}

#[test]
fn finite_and_interval_copy_destinations_preserve_disjoint_words() {
    for mask in [1, 0xffff] {
        for dynamic_width in [false, true] {
            let mut a = Asm::default();
            a.number(0x1234).number(0x80).op(0x52);
            if dynamic_width {
                a.number(32).op(0x35);
            } else {
                a.number(0xa0);
            }
            a.number(0)
                .number(0)
                .op(0x35)
                .number(mask)
                .op(0x16)
                .number(8)
                .op(0x1b)
                .number(0x1380)
                .op(0x01)
                .op(0x37);
            a.number(0x80).op(0x51).number(0).op(0x55).op(0x00);
            let input = a.finish();
            let artifact = checked(&input);
            assert_eq!(
                loads(&artifact),
                vec![&ValueFact::Constants(vec![U256::from(0x1234)])]
            );
            for offset in [0, 1, 2] {
                for width in [0, 3, 160] {
                    oracle(&input, &artifact, &[U256::from(offset), U256::from(width)]);
                }
            }
        }
    }
}

#[test]
fn finite_word_stores_are_weak_updates_and_keep_same_expression_forwarding() {
    let mut a = Asm::default();
    a.number(11).number(0x80).op(0x52);
    a.number(22).number(0xa0).op(0x52);
    a.number(7)
        .number(0)
        .op(0x35)
        .number(1)
        .op(0x16)
        .number(5)
        .op(0x1b)
        .number(0x80)
        .op(0x01)
        .op(0x80)
        .op(0x91)
        .op(0x90)
        .op(0x52);
    // Saved exact expression reads the just-written word, while either fixed
    // cell may instead retain its own previous value on the alternate choice.
    a.op(0x51).number(0).op(0x55);
    a.number(0x80).op(0x51).number(1).op(0x55);
    a.number(0xa0).op(0x51).number(2).op(0x55).op(0x00);
    let input = a.finish();
    let artifact = checked(&input);
    let expected = [vec![7], vec![7, 11], vec![7, 22]]
        .map(|v| ValueFact::Constants(v.into_iter().map(U256::from).collect()));
    assert_eq!(loads(&artifact), expected.iter().collect::<Vec<_>>());
    for choice in [0, 1, 2, 3] {
        oracle(&input, &artifact, &[U256::from(choice)]);
    }
}

#[test]
fn partial_unknown_and_zero_writes_never_keep_a_stale_overlapping_word() {
    for zero_width in [false, true] {
        let mut a = Asm::default();
        a.number(0x1234).number(0x80).op(0x52);
        if zero_width {
            a.number(0).number(0).number(0).op(0x35).op(0x37);
        } else {
            a.number(0)
                .number(0)
                .op(0x35)
                .number(1)
                .op(0x16)
                .number(0x80)
                .op(0x01)
                .op(0x53);
        }
        a.number(0x80).op(0x51).number(0).op(0x55).op(0x00);
        let input = a.finish();
        let artifact = checked(&input);
        let expected = if zero_width {
            ValueFact::Constants(vec![U256::from(0x1234)])
        } else {
            ValueFact::Unknown
        };
        assert_eq!(loads(&artifact)[0], &expected);
        for choice in [0, 1, 255] {
            oracle(&input, &artifact, &[U256::from(choice)]);
        }
    }
}
