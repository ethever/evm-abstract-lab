use super::support::{
    Assembly, Fixture, ROOT, address, complete, config, construct_world, entry, no_bad_branch,
    reachable, return_words, run, run_world,
};
use alloy_primitives::U256;
use evm_abstract::{
    Fork,
    analysis::{FrontierReason, OutcomeKind, Status},
    world::ByteArray,
};
use revm_bytecode::opcode;

/// Bounds come from EVM branches, never from choosing one concrete length.
fn bounded_input(code: &mut Assembly, lower: u16, upper: u16) {
    code.push(upper)
        .op(opcode::CALLDATASIZE)
        .op(opcode::GT)
        .jump("outside", true);
    code.push(lower)
        .op(opcode::CALLDATASIZE)
        .op(opcode::LT)
        .jump("outside", true);
}

fn copy_all_input(code: &mut Assembly) {
    code.op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .op(opcode::DUP1)
        .op(opcode::CALLDATACOPY);
}

#[test]
fn minimal_unknown_length_copy_keeps_eligible_completion_and_symbolic_length() {
    let analysis = run("366000803700");
    let completed: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(
        !completed.is_empty(),
        "retain in-range lengths instead of stopping every COPY"
    );
    let mut covers = [false; 3];
    for outcome in completed {
        assert_eq!(outcome.data.len().singleton(), Some(U256::ZERO));
        let frame = analysis.states()[outcome.state]
            .exit
            .as_ref()
            .unwrap()
            .active();
        for (covered, length) in covers.iter_mut().zip([0, 32, 512]) {
            *covered |= frame.memory.len().contains(U256::from(length));
        }
        assert!(
            frame.memory.work_size() < 4096,
            "keep unknown length as a compact range"
        );
    }
    assert!(
        covers.into_iter().all(|covered| covered),
        "zero and nonzero lengths survive"
    );
    // A logical-range policy may retain an explicit out-of-policy frontier.
    if analysis.status() == Status::Incomplete {
        assert!(
            analysis
                .frontiers()
                .iter()
                .all(|frontier| frontier.reason == FrontierReason::Memory)
        );
    }
}

#[test]
fn bounded_unenumerated_copy_preserves_a_word_constraint() {
    let mut code = Assembly::default();
    bounded_input(&mut code, 32, 512);
    code.load_input(0)
        .push(7)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("outside", true)
        .label("copy");
    copy_all_input(&mut code);
    code.op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .require_top(7)
        .return_top()
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    assert!(reachable(&analysis, ROOT, fixture.labels["copy"]));
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[0, 7]);
}

#[test]
fn bounded_large_logical_copy_does_not_expand_one_byte_at_a_time() {
    let mut code = Assembly::default();
    code.push_word(U256::from(1 << 20))
        .op(opcode::CALLDATASIZE)
        .op(opcode::GT)
        .jump("outside", true)
        .label("copy");
    copy_all_input(&mut code);
    code.op(opcode::STOP).label("outside").op(opcode::STOP);
    let fixture = code.finish();
    let mut policy = config();
    policy.max_memory_bytes = 1 << 20;
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &fixture.code)]),
        entry(ByteArray::unknown()),
        policy,
    );
    complete(&analysis);
    let copied: Vec<_> = analysis
        .states()
        .iter()
        .filter(|state| state.executed_pcs.contains(&fixture.labels["copy"]))
        .filter_map(|state| state.exit.as_ref())
        .collect();
    assert!(!copied.is_empty());
    assert!(
        copied
            .iter()
            .all(|payload| payload.active().memory.work_size() < 4096)
    );
    assert!(
        copied
            .iter()
            .any(|payload| payload.active().memory.len().contains(U256::from(1 << 20)))
    );
}

#[test]
fn copy_over_policy_retains_a_frontier_and_the_in_policy_paths() {
    let mut code = Assembly::default();
    bounded_input(&mut code, 0, 512);
    copy_all_input(&mut code);
    code.op(opcode::STOP).label("outside").op(opcode::STOP);
    let fixture = code.finish();
    let mut policy = config();
    policy.max_memory_bytes = 128;
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &fixture.code)]),
        entry(ByteArray::unknown()),
        policy,
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Memory)
    );
    assert!(
        analysis.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && analysis.states()[outcome.state]
                    .exit
                    .as_ref()
                    .is_some_and(|payload| payload.active().memory.len().contains(U256::from(128)))
        }),
        "an over-policy possibility must not erase the eligible COPY completion"
    );
}

#[test]
fn complete_unknown_input_delegate_return_preserves_content_length_and_byte_guards() {
    delegate_echo(false);
}

#[test]
fn complete_unknown_input_delegate_revert_preserves_content_length_and_byte_guards() {
    delegate_echo(true);
}

fn delegate_echo_fixture(revert: bool) -> (Fixture, Fixture) {
    let mut child = Assembly::default();
    child
        .load_input(0)
        .push(7)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .load_input(0)
        .push(31)
        .op(opcode::BYTE)
        .push(7)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true);
    copy_all_input(&mut child);
    child
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .op(if revert {
            opcode::REVERT
        } else {
            opcode::RETURN
        })
        .label("bad")
        .return_word(0xffff);
    let child = child.finish();

    let mut root = Assembly::default();
    bounded_input(&mut root, 32, 512);
    root.load_input(0)
        .push(7)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("outside", true);
    copy_all_input(&mut root);
    root.op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .push(0x200)
        .push(60_000)
        .op(opcode::DELEGATECALL);
    if revert {
        // Pre-entry call failure has empty data, unlike this child's REVERT.
        root.op(opcode::POP)
            .op(opcode::RETURNDATASIZE)
            .op(opcode::ISZERO)
            .jump("outside", true);
    } else {
        root.op(opcode::ISZERO).jump("outside", true);
    }
    root.op(opcode::RETURNDATASIZE)
        .op(opcode::CALLDATASIZE)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .op(opcode::RETURNDATASIZE)
        .op(opcode::PUSH0)
        .push(0x400)
        .op(opcode::RETURNDATACOPY)
        .push(0x400)
        .op(opcode::MLOAD)
        .require_top(7)
        .op(opcode::POP)
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .op(opcode::KECCAK256)
        .op(opcode::RETURNDATASIZE)
        .push(0x400)
        .op(opcode::KECCAK256)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .label("forwarded")
        .op(opcode::RETURNDATASIZE)
        .push(0x400)
        .op(if revert {
            opcode::REVERT
        } else {
            opcode::RETURN
        })
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let root = root.finish();
    (root, child)
}

fn delegate_echo(revert: bool) {
    let (root, child) = delegate_echo_fixture(revert);
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &root.code), (0x200, &child.code)]),
        entry(ByteArray::unknown()),
        config(),
    );
    complete(&analysis);
    no_bad_branch(&analysis, &child, 0x200);
    no_bad_branch(&analysis, &root, ROOT);
    let forwarded: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| {
            analysis.states()[outcome.state]
                .executed_pcs
                .contains(&root.labels["forwarded"])
        })
        .collect();
    assert!(!forwarded.is_empty(), "the full data route must complete");
    for length in [32, 33, 64, 255, 511, 512] {
        assert!(
            forwarded
                .iter()
                .any(|outcome| outcome.data.len().contains(U256::from(length))),
            "lost eligible length {length}"
        );
    }
    assert!(
        forwarded
            .iter()
            .any(|outcome| outcome.data.len().singleton().is_none()),
        "keep an unenumerated length range"
    );
    for outcome in forwarded {
        assert_eq!(
            outcome.kind,
            if revert {
                OutcomeKind::Revert
            } else {
                OutcomeKind::Return
            }
        );
    }
}

#[test]
fn concrete_delegate_echo_witnesses_validate_the_symbolic_fixture_bytecode() {
    for revert in [false, true] {
        let (root, child) = delegate_echo_fixture(revert);
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            let world = construct_world(fork, &[(ROOT, &root.code), (0x200, &child.code)]);
            for length in [32, 33, 64, 511, 512] {
                let mut input = vec![0; length];
                input[..32].copy_from_slice(&U256::from(7).to_be_bytes::<32>());
                for (offset, byte) in input.iter_mut().enumerate().skip(32) {
                    *byte = (offset % 251) as u8;
                }
                let entry = entry(ByteArray::exact(&input));
                let analysis = run_world(world.clone(), entry.clone(), config());
                complete(&analysis);
                let concrete = super::oracle::compare(&world, &entry, &analysis);
                assert_eq!(
                    concrete.kind,
                    if revert {
                        OutcomeKind::Revert
                    } else {
                        OutcomeKind::Return
                    }
                );
                assert_eq!(
                    concrete.output, input,
                    "fork={fork} revert={revert} length={length}"
                );
            }
        }
    }
}

#[test]
fn child_input_uses_the_selected_memory_window_instead_of_parent_calldata() {
    let mut child = Assembly::default();
    child
        .load_input(0)
        .require_top(9)
        .return_top()
        .label("bad")
        .return_word(0xffff);
    let child = child.finish();
    let mut root = Assembly::default();
    for (offset, expected) in [(0, 7), (32, 9)] {
        root.load_input(offset)
            .push(expected)
            .op(opcode::EQ)
            .op(opcode::ISZERO)
            .jump("outside", true);
    }
    root.push(64)
        .op(opcode::PUSH0)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATACOPY)
        .call(opcode::DELEGATECALL, 0x200, 0, (32, 32), (0x100, 32))
        .op(opcode::ISZERO)
        .jump("outside", true)
        .push(0x100)
        .op(opcode::MLOAD)
        .require_top(9)
        .return_top()
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let root = root.finish();
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &root.code), (0x200, &child.code)]),
        entry(ByteArray::unknown()),
        config(),
    );
    assert!(
        analysis
            .states()
            .iter()
            .any(|state| state.active().code_address == address(0x200))
    );
    no_bad_branch(&analysis, &root, ROOT);
    no_bad_branch(&analysis, &child, 0x200);
    return_words(&analysis, &[0, 9]);
}

#[test]
fn concrete_zero_length_copy_does_not_expand_memory() {
    let mut code = Assembly::default();
    code.op(opcode::PUSH0)
        .push_word(U256::MAX)
        .push_word(U256::MAX)
        .op(opcode::CALLDATACOPY)
        .op(opcode::MSIZE)
        .return_top();
    let world = construct_world(Fork::Osaka, &[(ROOT, &code.finish().code)]);
    let entry = entry(ByteArray::empty());
    let analysis = run_world(world.clone(), entry.clone(), config());
    return_words(&analysis, &[0]);
    let concrete = super::oracle::compare(&world, &entry, &analysis);
    assert_eq!(concrete.output, U256::ZERO.to_be_bytes::<32>());
}

#[test]
fn independent_unknown_inputs_cannot_be_proved_to_have_different_hashes() {
    let mut code = Assembly::default();
    for (input, output) in [(0, 0x80), (32, 0xc0)] {
        code.push(32)
            .push(input)
            .push(output)
            .op(opcode::CALLDATACOPY);
    }
    code.push(32)
        .push(0x80)
        .op(opcode::KECCAK256)
        .push(32)
        .push(0xc0)
        .op(opcode::KECCAK256)
        .op(opcode::EQ)
        .jump("equal", true)
        .label("different")
        .return_word(0)
        .label("equal")
        .return_word(1);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    complete(&analysis);
    for label in ["equal", "different"] {
        assert!(
            reachable(&analysis, ROOT, fixture.labels[label]),
            "{label} has a witness"
        );
    }
}
