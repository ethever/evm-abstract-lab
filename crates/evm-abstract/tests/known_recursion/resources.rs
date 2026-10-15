use super::{fixtures, oracle, support::*};
use evm_abstract::{
    analysis::{ExecutionConfig, FrontierReason, Status},
    world::ByteArray,
};

#[test]
fn nonrecursive_known_calls_preserve_exact_precision_with_cache_on_and_off() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::chain(B as u16), &[]);
    insert(&mut world, B, &fixtures::leaf(), &[(0, 9)]);
    for use_summaries in [true, false] {
        let analysis = run_config(
            world.clone(),
            entry(ByteArray::empty()),
            ExecutionConfig {
                use_summaries,
                ..ExecutionConfig::default()
            },
        );
        complete(&analysis);
        assert!(analysis.procedure_summaries().is_empty());
        for outcome in returns(&analysis) {
            only(&output_word(outcome, 0), &[9]);
        }
        oracle::execute(&world, &entry(ByteArray::empty())).covers(&analysis);
    }
}

#[test]
fn analysis_depth_before_the_first_child_is_a_real_typed_frontier() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::chain(B as u16), &[]);
    insert(&mut world, B, &fixtures::leaf(), &[(0, 9)]);
    let analysis = run_config(
        world,
        entry(ByteArray::empty()),
        ExecutionConfig {
            max_call_depth: 1,
            ..ExecutionConfig::default()
        },
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(!analysis.frontiers().is_empty());
    assert!(
        analysis
            .frontiers()
            .iter()
            .all(|f| f.reason == FrontierReason::CallDepth)
    );
    assert!(analysis.procedure_summaries().is_empty());
}

#[test]
fn configured_analysis_depth_limits_a_nonrepeating_known_chain() {
    let mut world = new_world();
    for index in 0..9 {
        let owner = A + index;
        let mut code = Assembly::default();
        code.call(revm_bytecode::opcode::CALL, (owner + 1) as u16, 0, 0, 0)
            .op(revm_bytecode::opcode::POP)
            .op(revm_bytecode::opcode::STOP);
        insert(&mut world, owner, &code.finish(), &[]);
    }
    insert(&mut world, A + 9, &fixtures::leaf(), &[(0, 42)]);
    let analysis = run_config(
        world,
        entry(ByteArray::empty()),
        ExecutionConfig {
            max_call_depth: 8,
            ..ExecutionConfig::default()
        },
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::CallDepth)
    );
    assert!(
        !analysis
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::Work)
    );
    assert!(
        analysis
            .frontiers()
            .iter()
            .all(|f| f.reason == FrontierReason::CallDepth)
    );
    assert!(analysis.procedure_summaries().is_empty());
}

#[test]
fn an_explicit_work_cutoff_is_not_reported_as_recursive_closure() {
    let mut world = new_world();
    insert(&mut world, A, &fixtures::counter(A as u16), &[]);
    let analysis = run_config(
        world,
        entry(unknown_input()),
        ExecutionConfig {
            max_work: 32_000,
            ..ExecutionConfig::default()
        },
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::Work)
    );
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|record| !record.closed)
    );
}

#[test]
fn complex_symbolic_recursion_retains_real_frontiers_under_default_work() {
    let mut growing = new_world();
    insert(&mut growing, A, &fixtures::growth(), &[(0, 0), (1, 77)]);
    let mut static_context = new_world();
    insert(
        &mut static_context,
        A,
        &fixtures::context_counter(revm_bytecode::opcode::STATICCALL, B as u16),
        &[(0, 5)],
    );
    insert(
        &mut static_context,
        B,
        &fixtures::context_counter(revm_bytecode::opcode::STATICCALL, A as u16),
        &[(0, 9)],
    );
    for world in [growing, static_context] {
        // This is the same complete input/model used by the100M functional
        // tests. An inner closed relation cannot close unfinished outer work.
        let analysis = run(world, entry(unknown_input()));
        assert_eq!(analysis.config().max_work, 20_000_000);
        assert_eq!(analysis.config().max_call_depth, 32);
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(!analysis.frontiers().is_empty());
        assert!(
            analysis
                .frontiers()
                .iter()
                .any(|f| f.reason == FrontierReason::Work)
        );
        assert!(
            analysis
                .frontiers()
                .iter()
                .all(|f| matches!(f.reason, FrontierReason::Work | FrontierReason::SummaryWork))
        );
        assert!(!analysis.procedure_summaries().is_empty());
    }
}
