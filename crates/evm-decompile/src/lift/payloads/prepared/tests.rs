use super::{Memory, start};
use crate::{
    Completion, Config, Fork, Input, Ir, LiftedConstructionKind, StageKind, ValueKind, decompile,
};
use std::{cell::Cell, collections::BTreeMap};

fn source(extra_failed_read: bool) -> Ir {
    let mut bytes = evm_bytecode::parse_hex(include_str!(
        "../../../../tests/fixtures/call-results/delegate-precopy-return.hex"
    ))
    .unwrap();
    if extra_failed_read {
        let failure = bytes.iter().rposition(|byte| *byte == 0x5b).unwrap();
        // This stack-neutral observation is after the jump target, so the
        // original branch address and terminal memory range stay unchanged.
        bytes.splice(failure + 1..failure + 1, [0x3d, 0x50]);
    }
    let artifact = decompile(&Input::new(bytes, Fork::Osaka), &Config::default());
    artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap()
        .ir
        .clone()
}

#[test]
fn only_a_required_failed_path_size_read_belongs_to_the_consumer() {
    for extra_read in [false, true] {
        let ir = &source(extra_read);
        let projection = crate::lift::project(ir, |_| true).unwrap();
        let program = &projection.program;
        let proof = &program.revert_data[0];
        let block = ir.blocks.iter().find(|b| b.id == proof.block).unwrap();
        let body = program.blocks.iter().find(|b| b.block == block.id).unwrap();
        let constants: BTreeMap<_, _> = ir
            .values
            .iter()
            .filter_map(|v| match v.kind {
                ValueKind::Constant(word) => Some((v.id, word)),
                _ => None,
            })
            .collect();
        let dependencies =
            super::super::construction::data_construction_dependencies(block, proof, &constants);
        let size_read = block.operations.iter().find(|o| o.opcode == 0x3d).unwrap();
        assert!(!size_read.effects.is_pure());
        assert!(!super::super::capsule_operation(size_read));
        assert_eq!(dependencies.contains(&size_read.id), !extra_read);
        let position = body
            .statements
            .iter()
            .position(|s| s.operation == size_read.id)
            .unwrap();
        let mut memory = Memory::new(ir, |_| true).unwrap();
        assert_eq!(
            start(&mut memory, block, body, proof, &dependencies),
            Some(if extra_read { position + 1 } else { 0 })
        );
        assert_eq!(
            program.controls.iter().any(|c| c.guard.is_some()),
            !extra_read
        );
    }
}

#[test]
fn refused_consumer_query_never_publishes_a_suffix() {
    let ir = &source(false);
    let projection = crate::lift::project(ir, |_| true).unwrap();
    let program = &projection.program;
    let proof = &program.revert_data[0];
    let block = ir.blocks.iter().find(|b| b.id == proof.block).unwrap();
    let body = program.blocks.iter().find(|b| b.block == block.id).unwrap();
    let allow = Cell::new(true);
    let mut memory = Memory::new(ir, |_| allow.get()).unwrap();
    allow.set(false);
    assert!(start(&mut memory, block, body, proof, &Default::default()).is_none());
    assert!(memory.refused);
}

#[test]
fn a_retained_read_of_a_newer_buffer_allows_a_shorter_terminal_suffix() {
    for second_call in [false, true] {
        // Preserve 32 bytes of A in memory. B has an empty output range and
        // therefore changes the current return buffer without changing them.
        let mut code = vec![0x5f, 0x5f, 0x5f, 0x5f, 0x60, 1, 0x5a, 0xf4, 0x50];
        code.extend([0x60, 32, 0x5f, 0x5f, 0x3e]);
        if second_call {
            code.extend([0x5f, 0x5f, 0x5f, 0x5f, 0x60, 2, 0x5a, 0xf4, 0x50]);
        }
        code.extend([0x5f, 0x35, 0x60, 0, 0x57, 0x00]);
        let target_operand = code.len() - 3;
        code[target_operand] = u8::try_from(code.len()).unwrap();
        // The range is exactly 32 even though its raw source expression reads
        // the current buffer. A read of B cannot enter A's lazy consumer.
        code.extend([0x5b, 0x3d, 0x5f, 0x16, 0x60, 32, 0x01, 0x5f, 0xfd]);
        let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
        assert_eq!(
            artifact.completion,
            Completion::Complete,
            "{:?}",
            artifact.diagnostics
        );
        let ir = &artifact.current().unwrap().ir;
        let program = ir.lifted.as_ref().unwrap();
        let plan = &program.constructions[0];
        assert_eq!(plan.kind, LiftedConstructionKind::PreparedConsumer);
        assert_eq!(plan.start > 0, second_call);
        assert_eq!(
            program.controls.iter().any(|c| c.guard.is_some()),
            !second_call
        );
        assert_eq!(super::super::verify(ir, program, &mut |_| true), Some(true));
    }
}
