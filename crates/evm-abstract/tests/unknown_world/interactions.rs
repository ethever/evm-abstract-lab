use super::{fixtures, support::*};
use alloy_primitives::U256;
use evm_abstract::{analysis::FrameCode, world::ByteArray};
use revm_bytecode::opcode;

#[test]
fn unknown_callee_can_succeed_and_continue_parent_without_direct_storage_havoc() {
    let code = fixtures::preserved_parent(opcode::CALL, false);
    let mut world = new_world();
    insert(&mut world, A, &code, &[]);
    let analysis = run(world, entry(ByteArray::empty()));
    let returns = returns(&analysis);
    assert!(
        returns
            .iter()
            .any(|o| output_word(o, 0).contains(U256::ONE)),
        "unknown callee success is missing"
    );
    assert!(reachable(&analysis, A, code.labels["resumed"]));
    for outcome in returns {
        only(&output_word(outcome, 32), &[42]);
        only(&read(&outcome.store, A, 0), &[42]);
    }
}

fn check_busy_x(nested: bool, use_summaries: bool) {
    let code = fixtures::busy_x(nested);
    let mut world = new_world();
    insert(&mut world, A, &code, &[]);
    let config = evm_abstract::analysis::ExecutionConfig {
        use_summaries,
        ..Default::default()
    };
    let analysis = run_config(world, entry(calldata(0)), config);
    complete(&analysis);
    assert!(!analysis.unknown_summaries().is_empty());
    assert!(
        analysis
            .unknown_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
    let returns = returns(&analysis);
    for outcome in &returns {
        only(&output_word(outcome, 0), &[0, 1, 2]);
        only(&output_word(outcome, 32), &[1]);
        only(&read(&outcome.store, A, 0), &[0]);
    }
    for x in [0, 1, 2] {
        assert!(
            returns
                .iter()
                .any(|o| output_word(o, 0).contains(U256::from(x))),
            "missing x={x} after repeated callbacks"
        );
    }
    assert!(
        analysis
            .unknown_summaries()
            .iter()
            .flat_map(|s| &s.procedures)
            .any(|p| p.frame.code_address == address(A) && p.frame.mode == FrameCode::Runtime),
        "callback coverage has no known A entry evidence"
    );
    if nested {
        assert!(reachable(&analysis, A, code.labels["three"]));
        assert!(
            analysis
                .unknown_summaries()
                .iter()
                .flat_map(|s| &s.procedures)
                .any(|p| p.frame.code_address == address(V) && p.frame.mode == FrameCode::Unknown),
            "the actual three entry did not create its nested unknown V relation"
        );
    }
    for label in ["one", "two"] {
        assert!(reachable(&analysis, A, code.labels[label]));
    }
}

#[test]
fn busy_x_closes_repeated_callbacks_at_zero_one_two_and_preserves_busy() {
    check_busy_x(false, true);
}

#[test]
fn busy_x_keeps_precision_with_nested_unknown_known_unknown_interactions() {
    check_busy_x(true, true);
}

#[test]
fn nested_busy_x_keeps_the_same_precision_and_closure_without_summary_cache() {
    check_busy_x(true, false);
}

#[test]
fn unknown_can_call_nonparent_c_and_new_reads_do_not_refresh_old_stack_value() {
    let a = fixtures::external_reader(false);
    let c = fixtures::external_cell();
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    insert(&mut world, C, &c, &[(0, 5), (1, 1)]);
    let analysis = run(world, entry(ByteArray::empty()));
    let returns = returns(&analysis);
    assert!(
        returns
            .iter()
            .any(|o| output_word(o, 32).contains(U256::from(9))),
        "unknown -> C write was not visible after return"
    );
    for outcome in returns {
        only(&output_word(outcome, 0), &[5]);
        only(&output_word(outcome, 32), &[5, 9]);
        only(&read(&outcome.store, C, 1), &[1]);
    }
    let ir = evm_abstract::ssa::build_partial_world(&analysis).unwrap();
    ir.verify(&analysis).unwrap();
}

#[test]
fn actual_c_entry_guard_prevents_unknown_interaction_from_writing_nine() {
    let a = fixtures::external_reader(false);
    let c = fixtures::external_cell();
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    insert(&mut world, C, &c, &[(0, 5), (1, 0)]);
    let analysis = run(world, entry(ByteArray::empty()));
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[5]);
        only(&output_word(outcome, 32), &[5]);
        only(&read(&outcome.store, C, 0), &[5]);
        only(&read(&outcome.store, C, 1), &[0]);
    }
}
