//! Finite memory reads must recover complete targets and preserve concrete reads.
use evm_bytecode::Program;
use evm_decompile::{
    Artifact, Cancellation, Completion, Config, Execution, ExecutionError, ExecutionLimits,
    ExecutionOutcome, Fork, Input, MemoryHost, Phase, Stage, StageKind, Terminator, U256,
    ValidationStatus, ValueFact, ValueKind, decompile, decompile_with_progress, execute_bytecode,
    execute_ir, execute_structured,
};
use std::collections::BTreeSet;

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
use assembly::Asm;

const LABELS: [&str; 4] = ["zero", "one", "two", "three"];
const ENTRIES: [usize; 8] = [0, 1, 1, 2, 2, 2, 2, 3];

#[derive(Clone, Copy)]
enum Mutation {
    None,
    UnknownWord,
    Overlap,
    Untouched,
}

fn table(mask: Option<u64>, mutation: Mutation, reads: usize) -> Input {
    let mut a = Asm::default();
    for (index, target) in ENTRIES.iter().enumerate() {
        if index == 7 && matches!(mutation, Mutation::Untouched) {
            continue;
        }
        a.target(LABELS[*target])
            .number(0x80 + 32 * index as u64)
            .op(0x52);
    }
    match mutation {
        Mutation::UnknownWord => {
            a.number(32).op(0x35).number(0x160).op(0x52);
        }
        Mutation::Overlap => {
            a.number(0).number(0x17f).op(0x53);
        }
        _ => {}
    }
    for index in 0..reads {
        a.number(0).op(0x35);
        if let Some(mask) = mask {
            a.number(mask).op(0x16);
        }
        a.number(5).op(0x1b).number(0x80).op(0x01).op(0x51);
        if index + 1 != reads {
            a.op(0x50);
        }
    }
    a.number(0xffff_ffff).op(0x16).op(0x56);
    for (index, label) in LABELS.iter().enumerate() {
        a.dest(label)
            .number(100 + index as u64)
            .number(0)
            .op(0x55)
            .op(0x00);
    }
    a.finish()
}

fn baseline(artifact: &Artifact) -> &Stage {
    artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .unwrap()
}

fn validated(artifact: &Artifact) {
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| { stage.validation.status == ValidationStatus::Passed }),
        "{:?}",
        artifact.diagnostics
    );
}

fn targets(input: &Input) -> BTreeSet<U256> {
    Program::decode_with_fork(&input.runtime_bytecode, input.fork)
        .unwrap()
        .jumpdest_blocks()
        .keys()
        .copied()
        .map(U256::from)
        .collect()
}

fn facts(artifact: &Artifact, opcode: u8) -> Vec<&ValueFact> {
    let ir = &baseline(artifact).ir;
    ir.blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter(|op| op.opcode == opcode)
        .map(|op| &ir.value(op.outputs[0]).unwrap().fact)
        .collect()
}

fn constants(fact: &ValueFact) -> BTreeSet<U256> {
    let ValueFact::Constants(values) = fact else {
        panic!("expected complete finite fact, got {fact:?}")
    };
    values.iter().copied().collect()
}

fn jumps(artifact: &Artifact) -> Vec<(&[evm_decompile::EntityId], bool, &[U256])> {
    baseline(artifact)
        .ir
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Jump {
                edges,
                unresolved,
                invalid_targets,
                ..
            } => Some((edges.as_slice(), *unresolved, invalid_targets.as_slice())),
            _ => None,
        })
        .collect()
}

fn host(input: &Input, words: &[U256]) -> MemoryHost {
    MemoryHost::new(
        input,
        words
            .iter()
            .flat_map(|word| word.to_be_bytes::<32>())
            .collect(),
    )
}

/// Direct bytecode execution is independent of the recovered SSA/control graph.
fn differential(
    input: &Input,
    artifact: &Artifact,
    initial: MemoryHost,
) -> (Result<Execution, ExecutionError>, MemoryHost) {
    let limits = ExecutionLimits::default();
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits);
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut ir_host = initial.clone();
        let ir = execute_ir(stage, &mut ir_host, &limits);
        assert_eq!(
            raw.as_ref().map(|r| &r.outcome),
            ir.as_ref().map(|r| &r.outcome),
            "IR {:?}",
            stage.kind
        );
        assert_eq!(
            raw.as_ref().map(Execution::observations),
            ir.as_ref().map(Execution::observations)
        );
        assert_eq!(raw_host, ir_host);
        if stage.ir.structured.is_some() {
            let mut source_host = initial.clone();
            let source = execute_structured(stage, &mut source_host, &limits);
            assert_eq!(
                raw.as_ref().map(|r| &r.outcome),
                source.as_ref().map(|r| &r.outcome),
                "AST {:?}",
                stage.kind
            );
            assert_eq!(
                raw.as_ref().map(Execution::observations),
                source.as_ref().map(Execution::observations)
            );
            assert_eq!(raw_host, source_host);
        }
    }
    (raw, raw_host)
}

fn unresolved_prefix(input: &Input, artifact: &Artifact, words: &[U256]) {
    let initial = host(input, words);
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, &ExecutionLimits::default()).unwrap();
    let jump = raw
        .trace
        .iter()
        .position(|step| step.opcode == 0x56)
        .unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut ir_host = initial.clone();
        let ir = execute_ir(stage, &mut ir_host, &ExecutionLimits::default()).unwrap();
        assert!(
            matches!(ir.outcome, ExecutionOutcome::PartialJump { target: Some(target) } if target == raw.trace[jump].inputs[0])
        );
        assert_eq!(ir.observations(), raw.observations()[..=jump]);
        assert_eq!(ir_host.memory, raw_host.memory);
    }
}

#[test]
fn eight_address_table_recovers_all_four_destinations_and_each_concrete_dispatch() {
    let input = table(Some(7), Mutation::None, 1);
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(constants(facts(&artifact, 0x51)[0]), targets(&input));
    let dispatch = jumps(&artifact);
    assert_eq!(dispatch.len(), 1);
    assert_eq!(dispatch[0].0.len(), 4);
    assert!(!dispatch[0].1 && dispatch[0].2.is_empty());
    for (index, selected) in ENTRIES.iter().enumerate() {
        let (_, observed) = differential(&input, &artifact, host(&input, &[U256::from(index)]));
        assert_eq!(observed.storage[&U256::ZERO], U256::from(100 + selected));
    }
}

#[test]
fn any_unknown_or_overwritten_candidate_keeps_the_entire_dispatch_unresolved() {
    for mutation in [Mutation::UnknownWord, Mutation::Overlap] {
        let input = table(Some(7), mutation, 1);
        let artifact = decompile(&input, &Config::default());
        validated(&artifact);
        assert_eq!(artifact.completion, Completion::Partial);
        assert_eq!(*facts(&artifact, 0x51)[0], ValueFact::Unknown);
        assert!(
            jumps(&artifact)
                .iter()
                .all(|(edges, unresolved, _)| *unresolved && edges.is_empty())
        );
        let target = *targets(&input).first().unwrap();
        for index in [0, 7] {
            unresolved_prefix(&input, &artifact, &[U256::from(index), target]);
        }
    }
}

#[test]
fn untouched_candidate_contributes_zero_instead_of_disappearing_from_the_union() {
    let input = table(Some(7), Mutation::Untouched, 1);
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Complete);
    let mut expected = targets(&input);
    // The only entry for the fourth destination was deliberately not written.
    expected.pop_last();
    expected.insert(U256::ZERO);
    assert_eq!(constants(facts(&artifact, 0x51)[0]), expected);
    let dispatch = jumps(&artifact);
    assert_eq!(dispatch.len(), 1);
    assert!(!dispatch[0].1);
    assert_eq!(dispatch[0].2, [U256::ZERO]);
    for index in [0, 1, 3, 7] {
        let _ = differential(&input, &artifact, host(&input, &[U256::from(index)]));
    }
}

#[test]
fn same_trusted_symbolic_address_keeps_its_stored_observation_before_enumeration() {
    for finite in [true, false] {
        let mut a = Asm::default();
        a.number(0).op(0x35);
        if finite {
            a.number(1).op(0x16);
        }
        a.number(5)
            .op(0x1b)
            .number(0x80)
            .op(0x01)
            .op(0x80)
            .number(7)
            .op(0x90)
            .op(0x52)
            .op(0x51)
            .number(0)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = decompile(&input, &Config::default());
        validated(&artifact);
        assert_eq!(artifact.completion, Completion::Complete);
        assert_eq!(facts(&artifact, 0x51)[0].singleton(), Some(U256::from(7)));
        for index in [0, 1, 3] {
            let (_, observed) = differential(&input, &artifact, host(&input, &[U256::from(index)]));
            assert_eq!(observed.storage[&U256::ZERO], U256::from(7));
        }
    }
}

fn input_dependent_cells(mask: u64, compare: bool) -> Input {
    let mut a = Asm::default();
    for (source, address) in [(32, 0x80), (64, 0xa0)] {
        a.number(source)
            .op(0x35)
            .number(mask)
            .op(0x16)
            .number(address)
            .op(0x52);
    }
    a.number(0)
        .op(0x35)
        .number(1)
        .op(0x16)
        .number(5)
        .op(0x1b)
        .number(0x80)
        .op(0x01)
        .op(0x51);
    if compare {
        a.number(256).op(0x90).op(0x10);
    }
    a.number(0).op(0x55).op(0x00);
    a.finish()
}

#[test]
fn different_input_dependent_cells_do_not_gain_a_false_shared_identity() {
    let input = input_dependent_cells(1, false);
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(
        constants(facts(&artifact, 0x51)[0]),
        BTreeSet::from([U256::ZERO, U256::from(1)])
    );
    for selected in [0, 1] {
        for (left, right) in [(0, 1), (1, 0)] {
            let (_, observed) = differential(
                &input,
                &artifact,
                host(
                    &input,
                    &[U256::from(selected), U256::from(left), U256::from(right)],
                ),
            );
            assert_eq!(
                observed.storage[&U256::ZERO],
                U256::from(if selected == 0 { left } else { right })
            );
        }
    }
    let ir = &baseline(&artifact).ir;
    for load in ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter(|o| o.opcode == 0x51)
    {
        assert!(
            matches!(ir.value(load.outputs[0]).unwrap().kind, ValueKind::Operation { operation, index: 0 } if operation == load.id)
        );
    }
}

#[test]
fn joined_memory_values_do_not_prove_equality_to_one_original_input() {
    let mut a = Asm::default();
    // Retain the first input on the stack as well as storing it. A false
    // expression identity on the joined read would erase the unequal arm.
    a.number(32)
        .op(0x35)
        .number(1)
        .op(0x16)
        .op(0x80)
        .number(0x80)
        .op(0x52)
        .number(64)
        .op(0x35)
        .number(1)
        .op(0x16)
        .number(0xa0)
        .op(0x52)
        .number(0)
        .op(0x35)
        .number(1)
        .op(0x16)
        .number(5)
        .op(0x1b)
        .number(0x80)
        .op(0x01)
        .op(0x51)
        .op(0x14)
        .target("equal")
        .op(0x57)
        .number(20)
        .number(0)
        .op(0x55)
        .op(0x00)
        .dest("equal")
        .number(10)
        .number(0)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(
        constants(facts(&artifact, 0x14)[0]),
        BTreeSet::from([U256::ZERO, U256::from(1)])
    );
    for selected in [0, 1] {
        for (left, right) in [(0, 1), (1, 0), (1, 1)] {
            let (_, observed) = differential(
                &input,
                &artifact,
                host(
                    &input,
                    &[U256::from(selected), U256::from(left), U256::from(right)],
                ),
            );
            let equal = selected == 0 || left == right;
            assert_eq!(
                observed.storage[&U256::ZERO],
                U256::from(if equal { 10 } else { 20 })
            );
        }
    }
}

#[test]
fn joining_nonfinite_words_retains_their_common_byte_bound() {
    let input = input_dependent_cells(255, true);
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(*facts(&artifact, 0x51)[0], ValueFact::Unknown);
    assert_eq!(facts(&artifact, 0x10)[0].singleton(), Some(U256::from(1)));
    for selected in [0, 1] {
        let (_, observed) = differential(
            &input,
            &artifact,
            host(&input, &[U256::from(selected), U256::from(511), U256::MAX]),
        );
        assert_eq!(observed.storage[&U256::ZERO], U256::from(1));
    }
}

#[test]
fn unaligned_exact_words_work_but_shifted_overlapping_reads_remain_unknown() {
    for exact in [true, false] {
        let mut a = Asm::default();
        let base = if exact { 0x81 } else { 0x80 };
        a.number(11)
            .number(base)
            .op(0x52)
            .number(22)
            .number(base + 32)
            .op(0x52)
            .number(0)
            .op(0x35)
            .number(1)
            .op(0x16)
            .number(5)
            .op(0x1b)
            .number(0x81)
            .op(0x01)
            .op(0x51)
            .number(0)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = decompile(&input, &Config::default());
        validated(&artifact);
        if exact {
            assert_eq!(
                constants(facts(&artifact, 0x51)[0]),
                BTreeSet::from([U256::from(11), U256::from(22)])
            );
        } else {
            assert_eq!(*facts(&artifact, 0x51)[0], ValueFact::Unknown);
        }
        for selected in [0, 1] {
            let _ = differential(&input, &artifact, host(&input, &[U256::from(selected)]));
        }
    }
}

#[test]
fn unrepresentable_candidate_preserves_unknown_and_the_original_memory_fault() {
    let mut a = Asm::default();
    a.number(7)
        .number(0x80)
        .op(0x52)
        .number(0)
        .op(0x35)
        .number(1)
        .op(0x16)
        .word(U256::MAX - U256::from(0x80))
        .op(0x02)
        .number(0x80)
        .op(0x01)
        .op(0x51)
        .number(0)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    assert_eq!(*facts(&artifact, 0x51)[0], ValueFact::Unknown);
    let (_, observed) = differential(&input, &artifact, host(&input, &[U256::ZERO]));
    assert_eq!(observed.storage[&U256::ZERO], U256::from(7));
    let (outcome, _) = differential(&input, &artifact, host(&input, &[U256::from(1)]));
    assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
}

#[test]
fn over_capacity_or_open_address_covers_do_not_publish_a_subset_of_targets() {
    for (mask, capacity) in [(Some(7), 4), (None, 8)] {
        let input = table(mask, Mutation::None, 1);
        let mut config = Config::default();
        config.limits.max_constant_set = capacity;
        let artifact = decompile(&input, &config);
        validated(&artifact);
        assert_eq!(artifact.completion, Completion::Partial);
        assert_eq!(*facts(&artifact, 0x51)[0], ValueFact::Unknown);
        assert!(
            jumps(&artifact)
                .iter()
                .all(|(edges, unresolved, _)| edges.is_empty() && *unresolved)
        );
        unresolved_prefix(&input, &artifact, &[U256::from(1)]);
    }
}

#[test]
fn recovery_refusal_and_cancellation_never_publish_a_partial_finite_target_union() {
    let input = table(Some(7), Mutation::None, 128);
    let mut recovery_work = vec![];
    let complete = decompile_with_progress(&input, &Config::default(), &Cancellation::new(), |p| {
        if p.phase == Phase::Recovery {
            recovery_work.push(p.work);
        }
    });
    assert_eq!(
        complete.completion,
        Completion::Complete,
        "{:?}",
        complete.diagnostics
    );
    assert!(
        recovery_work.len() >= 3,
        "exercise paid work within recovery"
    );
    let cutoff = recovery_work[recovery_work.len() / 2];
    let cancel = Cancellation::new();
    let cancelled = decompile_with_progress(&input, &Config::default(), &cancel, |p| {
        if p.phase == Phase::Recovery && p.work >= cutoff {
            cancel.cancel();
        }
    });
    assert_eq!(cancelled.completion, Completion::Cancelled);
    let mut config = Config::default();
    config.limits.max_work = cutoff;
    let limited = decompile(&input, &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert!(limited.statistics.work <= cutoff);
    for attempt in [&cancelled, &limited] {
        validated(attempt);
        assert_eq!(attempt.current().unwrap().kind, StageKind::Decoded);
        assert!(
            attempt
                .stages
                .iter()
                .all(|stage| stage.kind == StageKind::Decoded)
        );
        assert!(
            attempt
                .events
                .iter()
                .all(|event| event.rule != "local_recovery")
        );
    }
}

#[test]
fn finite_storage_addresses_keep_the_existing_sload_and_tload_fallback() {
    for (store, load) in [(0x55, 0x54), (0x5d, 0x5c)] {
        let mut a = Asm::default();
        a.target("zero")
            .number(0)
            .op(store)
            .target("one")
            .number(1)
            .op(store)
            .number(0)
            .op(0x35)
            .number(1)
            .op(0x16)
            .op(load)
            .op(0x56)
            .dest("zero")
            .op(0x00)
            .dest("one")
            .op(0x00);
        let input = a.finish();
        let artifact = decompile(&input, &Config::default());
        validated(&artifact);
        assert_eq!(artifact.completion, Completion::Partial);
        assert_eq!(*facts(&artifact, load)[0], ValueFact::Unknown);
        unresolved_prefix(&input, &artifact, &[U256::from(1)]);
    }
}
