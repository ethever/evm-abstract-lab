//! Sharing must retain the complete captured program and its public value shape.

use super::{Fork, Program};
use revm_bytecode::opcode;
use serde_json::json;
use std::sync::Arc;

#[test]
fn paid_program_equality_has_a_positive_shortcut_and_full_value_fallback() {
    use crate::resource::{CloneError, WorkBudget};
    let bytes = [opcode::JUMPDEST, opcode::PUSH0, opcode::STOP];
    let left = Program::decode(&bytes).unwrap();
    let independent = Program::decode(&bytes).unwrap();
    let mut shared = WorkBudget::new(16);
    assert!(left.eq_with_payment(&left.clone(), &mut shared).unwrap());
    assert_eq!(shared.used(), 16);
    assert_eq!(
        left.eq_with_payment(&left, &mut WorkBudget::new(15)),
        Err(CloneError::Work)
    );
    assert!(!left.shares_capture(&independent));
    let complete_fee = 16 + 64 + 64 * (left.byte_len() + independent.byte_len());
    let mut complete = WorkBudget::new(complete_fee);
    assert!(left.eq_with_payment(&independent, &mut complete).unwrap());
    assert_eq!(complete.used(), complete_fee);
    assert_eq!(
        left.eq_with_payment(&independent, &mut WorkBudget::new(complete_fee - 1)),
        Err(CloneError::Work)
    );
    for other in [
        Program::decode_with_fork(&bytes, Fork::Prague).unwrap(),
        Program::decode(&[opcode::JUMPDEST, opcode::PUSH0, opcode::INVALID]).unwrap(),
    ] {
        assert!(
            !left
                .eq_with_payment(&other, &mut WorkBudget::new(1_000_000))
                .unwrap()
        );
    }
    let payer = analysis_control::Cancellation::new();
    let mut cancelled = analysis_control::with_cancellation(&payer, || WorkBudget::new(1_000_000));
    payer.cancel();
    assert_eq!(
        left.eq_with_payment(&independent, &mut cancelled),
        Err(CloneError::Cancelled)
    );
    let ambient = analysis_control::Cancellation::new();
    let mut live = WorkBudget::new(1_000_000);
    ambient.cancel();
    assert_eq!(
        analysis_control::with_cancellation(&ambient, || left
            .eq_with_payment(&independent, &mut live)),
        Err(CloneError::Cancelled)
    );
}

#[test]
fn cloned_programs_share_all_decoded_data_and_keep_the_captured_source() {
    let captured = [opcode::PUSH1, 1, opcode::JUMPDEST, opcode::STOP];
    let mut source = captured;
    let program = Program::decode(&source).unwrap();
    assert!(program.data.bytes_digest.get().is_none());
    let clones: Vec<_> = (0..64).map(|_| program.clone()).collect();
    let expected_digest = alloy_primitives::keccak256(captured);
    assert_eq!(clones[0].bytes_digest(), expected_digest);
    for clone in &clones {
        assert!(Arc::ptr_eq(&program.data, &clone.data));
        assert!(std::ptr::eq(program.bytes(), clone.bytes()));
        assert!(std::ptr::eq(program.blocks(), clone.blocks()));
        assert_eq!(clone.data.bytes_digest.get(), Some(&expected_digest));
        assert_eq!(clone.bytes_digest(), expected_digest);
        assert!(std::ptr::eq(
            program.jumpdest_blocks(),
            clone.jumpdest_blocks()
        ));
        for (original, shared) in program.blocks().iter().zip(clone.blocks()) {
            assert!(std::ptr::eq(
                original.instructions.as_slice(),
                shared.instructions.as_slice()
            ));
        }
    }
    // Sharing owned decoded data must not retain mutable caller input or depend
    // on the lifetime of the original handle.
    source.fill(opcode::INVALID);
    drop(program);
    let retained = clones.last().unwrap();
    assert_eq!(retained.bytes(), &captured);
    assert_eq!(retained.bytes_digest(), expected_digest);
    assert_eq!(retained.byte_len(), captured.len());
    assert_eq!(retained.fork(), Fork::Osaka);
    assert_eq!(retained.jumpdest_blocks().get(&2), Some(&1));
    assert_eq!(
        retained.blocks()[0].instructions[0].immediate,
        Some(alloy_primitives::U256::from(1))
    );
}

#[test]
fn program_serialization_keeps_the_original_unwrapped_shape() {
    let program = Program::decode_with_fork(
        &[opcode::STOP, opcode::JUMPDEST, opcode::STOP],
        Fork::Cancun,
    )
    .unwrap();
    let expected = json!({
        "fork": "cancun",
        "byte_len": 3,
        "blocks": [
            {
                "id": 0,
                "start_pc": 0,
                "instructions": [{"pc": 0, "opcode": 0, "immediate": null, "size": 1}]
            },
            {
                "id": 1,
                "start_pc": 1,
                "instructions": [
                    {"pc": 1, "opcode": 91, "immediate": null, "size": 1},
                    {"pc": 2, "opcode": 0, "immediate": null, "size": 1}
                ]
            }
        ]
    });
    assert_eq!(serde_json::to_value(&program).unwrap(), expected);
    program.bytes_digest();
    assert_eq!(serde_json::to_value(program.clone()).unwrap(), expected);
}

#[test]
fn debug_keeps_the_original_derived_shape_in_both_formats() {
    #[derive(Debug)]
    #[expect(
        dead_code,
        reason = "fields are read by the derived Debug implementation"
    )]
    struct Program {
        fork: Fork,
        byte_len: usize,
        bytes: Vec<u8>,
        blocks: Vec<super::BasicBlock>,
        jumpdest_blocks: std::collections::BTreeMap<usize, usize>,
    }
    let decoded = super::Program::decode_with_fork(
        &[opcode::PUSH1, 1, opcode::JUMPDEST, opcode::STOP],
        Fork::Cancun,
    )
    .unwrap();
    // An owned view mirrors the complete pre-sharing derived Debug fields.
    let original = Program {
        fork: decoded.fork(),
        byte_len: decoded.byte_len(),
        bytes: decoded.bytes().to_vec(),
        blocks: decoded.blocks().to_vec(),
        jumpdest_blocks: decoded.jumpdest_blocks().clone(),
    };
    assert_eq!(format!("{decoded:?}"), format!("{original:?}"));
    decoded.bytes_digest();
    assert_eq!(format!("{decoded:#?}"), format!("{original:#?}"));
}

#[test]
fn equality_compares_captured_bytes_and_fork_across_independent_allocations() {
    let source = [opcode::PUSH1, 1, opcode::STOP];
    let program = Program::decode(&source).unwrap();
    let independently_decoded = Program::decode(&source).unwrap();
    assert!(!Arc::ptr_eq(&program.data, &independently_decoded.data));
    assert_eq!(program.bytes_digest(), alloy_primitives::keccak256(source));
    assert!(independently_decoded.data.bytes_digest.get().is_none());
    assert_eq!(program, independently_decoded);
    assert_eq!(program, program.clone());
    assert_ne!(
        program,
        Program::decode(&[opcode::PUSH1, 2, opcode::STOP]).unwrap()
    );
    assert_ne!(
        program,
        Program::decode_with_fork(&source, Fork::Prague).unwrap()
    );
    let missing_push_byte = Program::decode(&[opcode::PUSH1]).unwrap();
    let explicit_zero = Program::decode(&[opcode::PUSH1, 0]).unwrap();
    assert_eq!(missing_push_byte.blocks(), explicit_zero.blocks());
    assert_ne!(missing_push_byte, explicit_zero);
    assert_ne!(missing_push_byte.bytes(), explicit_zero.bytes());
    assert_eq!(
        missing_push_byte.bytes_digest(),
        alloy_primitives::keccak256([opcode::PUSH1])
    );
    assert_eq!(
        explicit_zero.bytes_digest(),
        alloy_primitives::keccak256([opcode::PUSH1, 0])
    );
    assert_ne!(
        missing_push_byte.bytes_digest(),
        explicit_zero.bytes_digest()
    );

    // Fork identity also controls decoding/validity for the same opcode bytes.
    let fork_sensitive = [opcode::CLZ, opcode::STOP];
    let osaka = Program::decode_with_fork(&fork_sensitive, Fork::Osaka).unwrap();
    let prague = Program::decode_with_fork(&fork_sensitive, Fork::Prague).unwrap();
    assert_ne!(osaka, prague);
    assert_eq!(osaka.bytes_digest(), prague.bytes_digest());
    assert!(osaka.blocks()[0].instructions[0].is_valid());
    assert!(!prague.blocks()[0].instructions[0].is_valid());
}
