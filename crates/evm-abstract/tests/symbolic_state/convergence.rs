use super::support::{
    Assembly, ROOT, complete, config, construct_world, entry, no_bad_branch, reachable,
    return_words, run_world, word,
};
use alloy_primitives::U256;
use evm_abstract::{
    Fork,
    analysis::{FrontierReason, OutcomeKind, Status},
    domain::Domain,
    world::ByteArray,
};
use revm_bytecode::opcode;

#[test]
fn repeated_constant_memory_store_reaches_a_fixed_point() {
    let mut code = Assembly::default();
    code.label("loop")
        .push(7)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .load_input(0)
        .jump("loop", true)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .return_top();
    let mut policy = config();
    policy.analysis.max_states = 64;
    policy.analysis.max_transfers = 128;
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &code.finish().code)]),
        entry(ByteArray::unknown()),
        policy,
    );
    return_words(&analysis, &[7]);
    assert!(
        analysis.transfers() < 128,
        "stable content must close before its transfer cap"
    );
}

#[test]
fn continuously_changing_loop_keeps_completion_or_an_explicit_frontier() {
    let mut code = Assembly::default();
    code.label("loop")
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .push(1)
        .op(opcode::ADD)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .load_input(0)
        .jump("loop", true)
        .op(opcode::PUSH0)
        .op(opcode::MLOAD)
        .return_top();
    let mut policy = config();
    policy.analysis.max_states = 64;
    policy.analysis.max_transfers = 64;
    let analysis = run_world(
        construct_world(Fork::Osaka, &[(ROOT, &code.finish().code)]),
        entry(ByteArray::unknown()),
        policy,
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return
                && outcome
                    .data
                    .read_word(&word(0), Domain::default())
                    .contains(U256::from(1))),
        "the terminating calldata=0 witness must survive convergence/budget policy"
    );
    if analysis.status() == Status::Incomplete {
        assert!(!analysis.frontiers().is_empty());
        assert!(analysis.frontiers().iter().all(|frontier| matches!(
            frontier.reason,
            FrontierReason::Budget(_) | FrontierReason::Work | FrontierReason::Relations(_)
        )));
    } else {
        complete(&analysis);
    }
}

#[test]
fn summary_reuse_freshens_per_call_observations_and_preserves_history() {
    let mut root = Assembly::default();
    root.call(opcode::STATICCALL, 0x200, 0, (0, 0), (0x80, 32))
        .push(0x100)
        .op(opcode::MSTORE)
        .push(0x80)
        .op(opcode::MLOAD)
        .call(opcode::STATICCALL, 0x200, 0, (0, 0), (0xc0, 32))
        .push(0x120)
        .op(opcode::MSTORE);
    // Inspect success only after both calls, so no new caller path constraint
    // makes their callee inputs differ and silently bypasses summary replay.
    for status in [0x100, 0x120] {
        root.push(status)
            .op(opcode::MLOAD)
            .op(opcode::ISZERO)
            .jump("outside", true);
    }
    root.op(opcode::DUP1)
        .push(0x80)
        .op(opcode::MLOAD)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .push(0xc0)
        .op(opcode::MLOAD)
        .op(opcode::EQ)
        .jump("equal", true)
        .label("different")
        .return_word(0)
        .label("equal")
        .return_word(1)
        .label("outside")
        .return_word(2)
        .label("bad")
        .return_word(0xffff);
    let root = root.finish();
    // GAS is a fresh abstract observation in each call, unlike fixed calldata.
    // Equal and unequal readings are both possible under the declared gas model.
    let world = construct_world(
        Fork::Osaka,
        &[(ROOT, &root.code), (0x200, "5a5f5260205ff3")],
    );
    let analyses: Vec<_> = [false, true]
        .into_iter()
        .map(|use_summaries| {
            let mut policy = config();
            policy.use_summaries = use_summaries;
            (
                use_summaries,
                run_world(world.clone(), entry(ByteArray::empty()), policy),
            )
        })
        .collect();
    assert!(
        analyses[1].1.summary_stats().hits > 0,
        "this fixture must exercise actual reuse: {:?}",
        analyses[1].1.summary_stats()
    );
    for (use_summaries, analysis) in analyses {
        complete(&analysis);
        no_bad_branch(&analysis, &root, ROOT);
        for label in ["equal", "different"] {
            assert!(
                reachable(&analysis, ROOT, root.labels[label]),
                "fresh {label} observations survive"
            );
        }
        return_words(&analysis, &[0, 1, 2]);
        assert_eq!(analysis.summary_stats().hits > 0, use_summaries);
    }
}

#[test]
fn summary_reuse_observes_current_storage_without_rewriting_the_previous_return() {
    let mut root = Assembly::default();
    for (value, output) in [(7, 0x80), (9, 0xc0)] {
        root.push(value)
            .op(opcode::PUSH0)
            .op(opcode::SSTORE)
            .call(opcode::DELEGATECALL, 0x200, 0, (0, 0), (output, 32))
            .op(opcode::ISZERO)
            .jump("outside", true);
    }
    root.push(0x80)
        .op(opcode::MLOAD)
        .require_top(7)
        .op(opcode::POP)
        .push(0xc0)
        .op(opcode::MLOAD)
        .require_top(9)
        .return_top()
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let root = root.finish();
    let world = construct_world(
        Fork::Osaka,
        &[(ROOT, &root.code), (0x200, "5f545f5260205ff3")],
    );
    for use_summaries in [false, true] {
        let mut policy = config();
        policy.use_summaries = use_summaries;
        let entry = entry(ByteArray::empty());
        let analysis = run_world(world.clone(), entry.clone(), policy);
        return_words(&analysis, &[0, 9]);
        no_bad_branch(&analysis, &root, ROOT);
        assert_eq!(
            super::oracle::compare(&world, &entry, &analysis).output,
            U256::from(9).to_be_bytes::<32>()
        );
    }
}
