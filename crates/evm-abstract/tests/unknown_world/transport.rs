use super::{oracle, support::*};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::world::EvmByteArray;
use revm_bytecode::opcode;

pub(super) fn parent(require_one_byte: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .push_word(U256::from_be_bytes([0xaa; 32]))
        .mstore(32)
        .push_word(U256::from_be_bytes([0xbb; 32]))
        .mstore(64)
        .call(opcode::CALL, U as u16, 0, (0, 0), (32, 16))
        .mstore(0)
        .op(opcode::RETURNDATASIZE);
    if require_one_byte {
        a.op(opcode::DUP1)
            .push(1)
            .op(opcode::EQ)
            .op(opcode::ISZERO)
            .jump("wrong_length", true);
    }
    a.mstore(96).return_range(0, 128);
    a.label("callback").op(opcode::STOP);
    if require_one_byte {
        a.label("wrong_length").push(0).push(0).op(opcode::REVERT);
    }
    a.finish()
}

pub(super) fn child(size: u16, revert: bool) -> Fixture {
    let mut u = Assembly::default();
    u.push_word(U256::from_be_bytes([0x7b; 32]))
        .mstore(0)
        .push_word(U256::from_be_bytes([0x8c; 32]))
        .mstore(32)
        .push(size)
        .push(0)
        .op(if revert {
            opcode::REVERT
        } else {
            opcode::RETURN
        });
    u.finish()
}

#[test]
fn unknown_output_copy_preserves_parent_memory_outside_requested_prefix() {
    let a = parent(false);
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    let analysis = run(world.clone(), entry(EvmByteArray::<NumericValue>::empty()));
    complete_unknown(&analysis);
    let outcomes = returns(&analysis);
    assert!(
        outcomes
            .iter()
            .any(|o| output_word(o, 96).contains(U256::ONE)),
        "unknown nonempty returndata is missing"
    );
    for outcome in outcomes {
        for offset in 48..64 {
            only(
                &outcome.data.byte_at(offset, native::product().borrowed()),
                &[0xaa],
            );
        }
        for offset in 64..96 {
            only(
                &outcome.data.byte_at(offset, native::product().borrowed()),
                &[0xbb],
            );
        }
    }
    for (size, revert) in [(1, false), (1, true), (64, false), (64, true)] {
        let concrete = oracle::concrete_unknown(&world, U, &child(size, revert));
        let observed = oracle::execute(&concrete, &entry(EvmByteArray::<NumericValue>::empty()));
        assert_eq!(observed.output_word(0), U256::from(u64::from(!revert)));
        assert_eq!(observed.output_word(96), U256::from(size));
        observed.covers(&analysis);
    }
}

#[test]
fn one_byte_unknown_return_or_revert_keeps_the_uncopied_requested_suffix() {
    let a = parent(true);
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    let analysis = run_config(
        world.clone(),
        entry(EvmByteArray::<NumericValue>::empty()),
        evm_abstract::analysis::ExecutionConfig {
            // Preserve the byte/oracle contract under the complete Native tariff.
            max_work: 40_000_000,
            ..Default::default()
        },
    );
    complete_unknown(&analysis);
    let outcomes = returns(&analysis);
    for outcome in outcomes {
        only(&output_word(outcome, 96), &[1]);
        for offset in 33..64 {
            only(
                &outcome.data.byte_at(offset, native::product().borrowed()),
                &[0xaa],
            );
        }
        for offset in 64..96 {
            only(
                &outcome.data.byte_at(offset, native::product().borrowed()),
                &[0xbb],
            );
        }
    }
    for revert in [false, true] {
        let concrete = oracle::concrete_unknown(&world, U, &child(1, revert));
        let observed = oracle::execute(&concrete, &entry(EvmByteArray::<NumericValue>::empty()));
        assert_eq!(observed.data[32], 0x7b);
        assert!(observed.data[33..64].iter().all(|byte| *byte == 0xaa));
        assert_eq!(observed.output_word(96), U256::ONE);
        observed.covers(&analysis);
    }
}
