//! Actual bytecode covers the shared inline and retained-binding rule paths.
use super::oracle::certify;
use super::support::{Asm, calldata, checked};
use super::{assert_unchanged_witness, lifted, word_cases};
use evm_decompile::{
    Artifact, EntityRef, ExecutionError, ExecutionLimits, Input, LiftedStatement, MemoryHost,
    StageKind, U256, WordRewrite, execute_bytecode, execute_ir, execute_structured, render_text,
};

fn word(a: &mut Asm, value: U256) {
    a.op(0x7f);
    for byte in value.to_be_bytes::<32>() {
        a.op(byte);
    }
}
fn mask160() -> U256 {
    (U256::from(1) << 160) - U256::from(1)
}
fn masks(a: &mut Asm, first: U256, second: U256, reverse_inner: bool, reverse_outer: bool) {
    word(a, first);
    if reverse_inner {
        a.op(0x90);
    }
    a.op(0x16);
    word(a, second);
    if reverse_outer {
        a.op(0x90);
    }
    a.op(0x16);
}
fn fixture(first: U256, second: U256, cross: bool, orientation: (bool, bool)) -> Input {
    let mut a = Asm::default();
    a.push(10).op(0x54);
    masks(&mut a, first, second, orientation.0, orientation.1);
    if cross {
        a.target("consume").op(0x56).dest("consume");
    }
    a.op(0x5f).op(0x55).op(0x00);
    a.finish()
}
fn values() -> Vec<U256> {
    let mut values = word_cases();
    values.extend([
        mask160(),
        mask160() + U256::from(1),
        (U256::from(1) << 159),
        (U256::from(0x1234) << 192) | U256::from(0xabcd),
    ]);
    values
}
fn bound(artifact: &Artifact, opcode: u8) -> &LiftedStatement {
    let (stage, program) = lifted(artifact);
    let statement = program
        .blocks
        .iter()
        .flat_map(|b| &b.statements)
        .find(|s| {
            stage
                .ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .any(|op| op.id == s.operation && op.opcode == opcode)
                && !s.outputs.is_empty()
        })
        .expect("cross-block pure root retains its own source binding");
    let op = stage
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| op.id == statement.operation)
        .unwrap();
    assert_eq!(statement.outputs, op.outputs);
    assert_eq!(statement.inputs.len(), op.inputs.len());
    let origin = artifact
        .origin(EntityRef {
            stage: stage.id,
            entity: statement.id,
        })
        .unwrap();
    assert!(
        origin.source.contains(&op.source.unwrap()),
        "retained definition keeps its source primitive"
    );
    statement
}

#[test]
fn local_and_cross_block_equal_masks_share_one_algebra_without_changing_raw_words() {
    let mask = mask160();
    for cross in [false, true] {
        let input = fixture(mask, mask, cross, (false, false));
        let artifact = checked(&input);
        let before = serde_json::to_vec(&artifact).unwrap();
        let text = render_text(&artifact);
        assert!(!text.contains("evm.and("), "{text}");
        if cross {
            let statement = bound(&artifact, 0x16);
            let (stage, program) = lifted(&artifact);
            let index = program.render_index(&stage.ir);
            assert!(matches!(
                index.statement_word_rhs(statement, |_| true).unwrap(),
                Some(WordRewrite::Existing(_))
            ));
            let row = index.render_statement(statement, |_| true).unwrap();
            assert!(row.starts_with(&format!("v{} = ", statement.outputs[0].0)));
            assert_eq!(row.matches(&format!("0x{mask:x}")).count(), 1, "{row}");
        }
        for value in values() {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.storage.insert(U256::from(10), value);
            let (raw, host) = certify(&input, &artifact, host);
            assert_eq!(host.storage[&U256::ZERO], value & mask);
            assert_eq!(raw.trace.iter().filter(|s| s.opcode == 0x54).count(), 1);
            assert_eq!(
                raw.trace.iter().filter(|s| s.opcode == 0x16).count(),
                2,
                "display does not edit primitive history"
            );
        }
        assert_unchanged_witness(&artifact);
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
    }
}

#[test]
fn different_and_disjoint_masks_preserve_named_outputs_in_every_operand_orientation() {
    let low128 = (U256::from(1) << 128) - U256::from(1);
    for (first, second, expected) in [
        (mask160(), low128, low128),
        (low128, U256::MAX ^ low128, U256::ZERO),
    ] {
        for inner in [false, true] {
            for outer in [false, true] {
                let input = fixture(first, second, true, (inner, outer));
                let artifact = checked(&input);
                let (stage, program) = lifted(&artifact);
                let index = program.render_index(&stage.ir);
                if expected == U256::ZERO {
                    let raw = stage
                        .ir
                        .blocks
                        .iter()
                        .flat_map(|b| &b.operations)
                        .rfind(|op| op.opcode == 0x16)
                        .unwrap();
                    assert_eq!(
                        raw.replacement,
                        Some(U256::ZERO),
                        "known bits already prove the disjoint masks before lifting"
                    );
                    assert!(
                        !program
                            .blocks
                            .iter()
                            .flat_map(|b| &b.statements)
                            .any(|s| s.operation == raw.id)
                    );
                } else {
                    let statement = bound(&artifact, 0x16);
                    let recipe = index
                        .statement_word_rhs(statement, |_| true)
                        .unwrap()
                        .unwrap();
                    assert!(matches!(recipe,WordRewrite::Masked {mask,..} if mask==expected));
                }
                for value in values() {
                    let mut host = MemoryHost::new(&input, Vec::new());
                    host.storage.insert(U256::from(10), value);
                    let (_, host) = certify(&input, &artifact, host);
                    assert_eq!(host.storage[&U256::ZERO], value & expected);
                }
                assert_unchanged_witness(&artifact);
            }
        }
    }
}

#[test]
fn copy_and_zero_identities_respect_earlier_folding_and_keep_storage_observation() {
    for opcode in [0x16, 0x17, 0x18, 0x03] {
        let mut a = Asm::default();
        a.push(10)
            .op(0x54)
            .op(0x80)
            .op(opcode)
            .target("consume")
            .op(0x56)
            .dest("consume")
            .op(0x5f)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = checked(&input);
        let (stage, program) = lifted(&artifact);
        let index = program.render_index(&stage.ir);
        if matches!(opcode, 0x16 | 0x17) {
            let statement = bound(&artifact, opcode);
            let recipe = index
                .statement_word_rhs(statement, |_| true)
                .unwrap()
                .unwrap();
            assert!(matches!(recipe, WordRewrite::Existing(_)));
        } else {
            let raw = stage
                .ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .find(|op| op.opcode == opcode)
                .unwrap();
            assert_eq!(raw.replacement, Some(U256::ZERO));
            assert!(
                !program
                    .blocks
                    .iter()
                    .flat_map(|b| &b.statements)
                    .any(|s| s.operation == raw.id)
            );
        }
        for value in values() {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.storage.insert(U256::from(10), value);
            let (_, host) = certify(&input, &artifact, host);
            assert_eq!(
                host.storage[&U256::ZERO],
                if matches!(opcode, 0x16 | 0x17) {
                    value
                } else {
                    U256::ZERO
                }
            );
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn an_invariant_word_stays_available_across_phi_arms_and_loop_iterations() {
    for looping in [false, true] {
        let mut a = Asm::default();
        a.push(10).op(0x54);
        masks(&mut a, mask160(), mask160(), false, false);
        if looping {
            a.push(2)
                .target("header")
                .op(0x56)
                .dest("header")
                .op(0x80)
                .op(0x15)
                .target("done")
                .op(0x57)
                .push(1)
                .op(0x90)
                .op(0x03)
                .target("header")
                .op(0x56)
                .dest("done")
                .op(0x50);
        } else {
            a.op(0x5f)
                .op(0x35)
                .target("left")
                .op(0x57)
                .target("join")
                .op(0x56)
                .dest("left")
                .push(2)
                .push(9)
                .op(0x55)
                .target("join")
                .op(0x56)
                .dest("join");
        }
        a.op(0x5f).op(0x55).op(0x00);
        let input = a.finish();
        let artifact = checked(&input);
        let statement = bound(&artifact, 0x16);
        let (stage, program) = lifted(&artifact);
        let index = program.render_index(&stage.ir);
        assert!(
            index
                .statement_word_rhs(statement, |_| true)
                .unwrap()
                .is_some()
        );
        for choice in [0, 1] {
            for value in values() {
                let mut host = MemoryHost::new(&input, calldata(&[U256::from(choice)]));
                host.storage.insert(U256::from(10), value);
                let (raw, host) = certify(&input, &artifact, host);
                assert_eq!(host.storage[&U256::ZERO], value & mask160());
                assert_eq!(raw.trace.iter().filter(|s| s.opcode == 0x54).count(), 1);
            }
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn conflicting_phi_inputs_keep_distinct_storage_captures() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("left")
        .op(0x57)
        .push(10)
        .op(0x54);
    masks(&mut a, mask160(), mask160(), false, false);
    a.target("join").op(0x56).dest("left").push(11).op(0x54);
    masks(&mut a, mask160(), mask160(), false, false);
    a.target("join").op(0x56).dest("join");
    let join = a.labels["join"];
    a.op(0x5f).op(0x55).op(0x00);
    let input = a.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let block = stage
        .ir
        .blocks
        .iter()
        .find(|b| b.source.is_some_and(|s| s.pc == join))
        .unwrap();
    assert_eq!(
        program.block(block.id).unwrap().parameters.len(),
        1,
        "different primitive identities are not an alias copy"
    );
    for choice in [0, 1] {
        let mut host = MemoryHost::new(&input, calldata(&[U256::from(choice)]));
        host.storage.insert(U256::from(10), U256::MAX);
        host.storage.insert(U256::from(11), U256::from(0x1234));
        let (raw, host) = certify(&input, &artifact, host);
        assert_eq!(
            host.storage[&U256::ZERO],
            if choice == 0 {
                mask160()
            } else {
                U256::from(0x1234)
            }
        );
        assert_eq!(
            raw.trace.iter().filter(|s| s.opcode == 0x54).count(),
            1,
            "the unselected branch does not load storage"
        );
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn observed_old_word_is_not_reloaded_after_a_store_and_memory_fault_prefix_stays_exact() {
    let mut a = Asm::default();
    a.push(10).op(0x54);
    masks(&mut a, mask160(), mask160(), false, false);
    a.push(0x5a)
        .push(10)
        .op(0x55)
        .push(0x40)
        .op(0x51)
        .op(0x50)
        .target("consume")
        .op(0x56)
        .dest("consume")
        .op(0x5f)
        .op(0x55)
        .push(10)
        .op(0x54);
    masks(&mut a, mask160(), mask160(), false, false);
    a.push(1).op(0x55).op(0x00);
    let input = a.finish();
    let artifact = checked(&input);
    let statement = bound(&artifact, 0x16);
    let (stage, program) = lifted(&artifact);
    let index = program.render_index(&stage.ir);
    assert!(
        index
            .statement_word_rhs(statement, |_| true)
            .unwrap()
            .is_some()
    );
    for value in values() {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.storage.insert(U256::from(10), value);
        let (raw, host) = certify(&input, &artifact, host);
        assert_eq!(host.storage[&U256::ZERO], value & mask160());
        assert_eq!(host.storage[&U256::from(1)], U256::from(0x5a));
        let effects: Vec<_> = raw
            .trace
            .iter()
            .filter(|s| matches!(s.opcode, 0x54 | 0x55 | 0x51))
            .map(|s| s.opcode)
            .collect();
        assert_eq!(effects, [0x54, 0x55, 0x51, 0x55, 0x54, 0x55]);
    }
    let limits = ExecutionLimits {
        max_memory_bytes: 63,
        ..ExecutionLimits::default()
    };
    let mut raw_host = MemoryHost::new(&input, Vec::new());
    raw_host.storage.insert(U256::from(10), U256::MAX);
    let initial = raw_host.clone();
    assert_eq!(
        execute_bytecode(&input, &mut raw_host, &limits),
        Err(ExecutionError::MemoryLimit)
    );
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut graph = initial.clone();
        assert_eq!(
            execute_ir(stage, &mut graph, &limits),
            Err(ExecutionError::MemoryLimit)
        );
        assert_eq!(graph, raw_host);
        if stage.ir.structured.is_some() {
            let mut tree = initial.clone();
            assert_eq!(
                execute_structured(stage, &mut tree, &limits),
                Err(ExecutionError::MemoryLimit)
            );
            assert_eq!(tree, raw_host);
        }
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn retained_predicate_word_boundaries_keep_their_original_primitive() {
    let mut a = Asm::default();
    a.op(0x34)
        .op(0x5f)
        .op(0x14)
        .target("consume")
        .op(0x56)
        .dest("consume")
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = checked(&input);
    let statement = bound(&artifact, 0x14);
    let (stage, program) = lifted(&artifact);
    let index = program.render_index(&stage.ir);
    assert_eq!(index.statement_word_rhs(statement, |_| true).unwrap(), None);
    assert!(
        index
            .render_statement(statement, |_| true)
            .unwrap()
            .contains("evm.eq(")
    );
    for value in values() {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.context.insert(0x34, value);
        let (_, host) = certify(&input, &artifact, host);
        assert_eq!(
            host.storage[&U256::ZERO],
            U256::from(u64::from(value == U256::ZERO))
        );
    }
    assert_unchanged_witness(&artifact);
}
