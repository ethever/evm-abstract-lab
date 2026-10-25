//! Concrete byte semantics must survive a more precise symbolic representation.
use super::{
    oracle,
    support::{Assembly, ROOT, complete, config, construct_world, entry, no_bad_branch, run_world},
};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::{Fork, analysis::OutcomeKind, world::EvmByteArray};
use revm_bytecode::opcode;

#[test]
fn big_endian_unaligned_overlays_preserve_each_historical_word() {
    let input: Vec<_> = (1..=64).collect();
    let mut code = Assembly::default();
    code.load_input(0)
        .push(1)
        .op(opcode::MSTORE)
        .push(1)
        .op(opcode::MLOAD)
        .push(0xaa)
        .push(32)
        .op(opcode::MSTORE8)
        .push(1)
        .op(opcode::MLOAD)
        .push(96)
        .op(opcode::MSTORE)
        .load_input(32)
        .push(17)
        .op(opcode::MSTORE)
        .push(1)
        .op(opcode::MLOAD)
        .push(128)
        .op(opcode::MSTORE)
        .push(64)
        .op(opcode::MSTORE)
        .push(96)
        .push(64)
        .op(opcode::RETURN);
    let fixture = code.finish();
    let mut expected = input[..32].to_vec();
    expected.extend(&input[..31]);
    expected.push(0xaa);
    expected.extend(&input[..16]);
    expected.extend(&input[32..48]);
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let world = construct_world(fork, &[(ROOT, &fixture.code)]);
        let entry = entry(EvmByteArray::<NumericValue>::exact(&input));
        let analysis = run_world(world.clone(), entry.clone(), config());
        complete(&analysis);
        let concrete = oracle::compare(&world, &entry, &analysis);
        assert_eq!(concrete.data, expected);
        for outcome in analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
        {
            assert_eq!(
                outcome.data.exact_bytes(native::product().borrowed()),
                Some(expected.clone())
            );
        }
    }
}

#[test]
fn mcopy_overlaps_read_the_source_before_either_direction_is_written() {
    let input: Vec<_> = (0..16).collect();
    for (source, destination) in [(0, 2), (2, 0)] {
        let mut code = Assembly::default();
        code.push(16)
            .op(opcode::PUSH0)
            .op(opcode::PUSH0)
            .op(opcode::CALLDATACOPY)
            .push(8)
            .push(source)
            .push(destination)
            .op(opcode::MCOPY)
            .push(16)
            .op(opcode::PUSH0)
            .op(opcode::RETURN);
        let fixture = code.finish();
        let mut expected = input.clone();
        expected[usize::from(destination)..usize::from(destination) + 8]
            .copy_from_slice(&input[usize::from(source)..usize::from(source) + 8]);
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            let world = construct_world(fork, &[(ROOT, &fixture.code)]);
            let entry = entry(EvmByteArray::<NumericValue>::exact(&input));
            let analysis = run_world(world.clone(), entry.clone(), config());
            complete(&analysis);
            let concrete = oracle::compare(&world, &entry, &analysis);
            assert_eq!(concrete.data, expected);
            assert!(
                analysis
                    .outcomes()
                    .iter()
                    .filter(|outcome| outcome.kind == OutcomeKind::Return)
                    .all(|outcome| {
                        outcome
                            .data
                            .exact_bytes(native::product().borrowed())
                            .as_deref()
                            == Some(expected.as_slice())
                    })
            );
        }
    }
}

#[test]
fn zero_length_mcopy_with_maximal_offsets_keeps_msize_zero() {
    let mut code = Assembly::default();
    code.op(opcode::PUSH0)
        .push_word(U256::MAX)
        .push_word(U256::MAX)
        .op(opcode::MCOPY)
        .op(opcode::MSIZE)
        .return_top();
    let world = construct_world(Fork::Osaka, &[(ROOT, &code.finish().code)]);
    let entry = entry(EvmByteArray::<NumericValue>::empty());
    let analysis = run_world(world.clone(), entry.clone(), config());
    complete(&analysis);
    assert_eq!(oracle::compare(&world, &entry, &analysis).data, [0; 32]);
    assert!(
        analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .all(|outcome| {
                outcome
                    .data
                    .exact_bytes(native::product().borrowed())
                    .as_deref()
                    == Some(&[0; 32])
            })
    );
}

#[test]
fn call_output_overwrites_only_the_actual_return_prefix() {
    let original = U256::from_be_bytes([0x55; 32]);
    let mut expected = [0x55; 32];
    expected[..3].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    let mut root = Assembly::default();
    root.push_word(original)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .call(opcode::CALL, 0x200, 0, (0, 0), (0, 32))
        .op(opcode::ISZERO)
        .jump("outside", true)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .push_word(U256::from_be_bytes(expected))
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .push(32)
        .op(opcode::PUSH0)
        .op(opcode::RETURN)
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let root = root.finish();
    // MSTORE places PUSH3 in the last three bytes of its big-endian word.
    let child = "62aabbcc5f526003601df3";
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let world = construct_world(fork, &[(ROOT, &root.code), (0x200, child)]);
        let entry = entry(EvmByteArray::<NumericValue>::empty());
        let analysis = run_world(world.clone(), entry.clone(), config());
        complete(&analysis);
        let concrete = oracle::compare(&world, &entry, &analysis);
        assert_eq!(concrete.data, expected);
        no_bad_branch(&analysis, &root, ROOT);
    }
}

#[test]
fn returndata_zero_length_at_end_is_valid_but_past_end_and_overflow_fault() {
    for (source, size, expected_kind) in [
        (U256::from(1), 0, OutcomeKind::Return),
        (U256::from(2), 0, OutcomeKind::Failure),
        (U256::ZERO, 2, OutcomeKind::Failure),
        (U256::MAX, 1, OutcomeKind::Failure),
    ] {
        let mut root = Assembly::default();
        root.call(opcode::CALL, 0x200, 0, (0, 0), (0, 0))
            .op(opcode::ISZERO)
            .jump("outside", true)
            .push(size)
            .push_word(source)
            .push_word(if size == 0 { U256::MAX } else { U256::ZERO })
            .op(opcode::RETURNDATACOPY)
            .op(opcode::MSIZE)
            .return_top()
            .label("outside")
            .return_word(0);
        let root = root.finish();
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            let world = construct_world(fork, &[(ROOT, &root.code), (0x200, "60095f5360015ff3")]);
            let entry = entry(EvmByteArray::<NumericValue>::empty());
            let analysis = run_world(world.clone(), entry.clone(), config());
            complete(&analysis);
            let concrete = oracle::compare(&world, &entry, &analysis);
            assert_eq!(concrete.kind, expected_kind, "source={source} size={size}");
            if expected_kind == OutcomeKind::Return {
                assert_eq!(concrete.data, [0; 32]);
            } else {
                assert!(concrete.data.is_empty());
            }
        }
    }
}

#[test]
fn nonempty_memory_access_does_not_wrap_a_maximal_offset_to_zero() {
    let mut code = Assembly::default();
    code.push_word(U256::MAX).op(opcode::MLOAD).return_top();
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &code.finish().code)]),
        entry(EvmByteArray::<NumericValue>::empty()),
        config(),
    );
    complete(&analysis);
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind != OutcomeKind::Return)
    );
}
