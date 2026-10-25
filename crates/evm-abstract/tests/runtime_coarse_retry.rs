//! Real unknown-environment callbacks exercise the relative Runtime solver.
//! Internal retry flags and Runtime receipts remain the solver's private tests.

#[path = "support/native.rs"]
mod native;

#[path = "runtime_coarse_retry/fixture.rs"]
mod fixture;
#[path = "runtime_coarse_retry/oracle.rs"]
mod oracle;
#[path = "runtime_coarse_retry/support.rs"]
mod support;
#[path = "runtime_coarse_retry/witness.rs"]
mod witness;

use evm_abstract::{
    U256,
    analysis::{DiagnosticKind, FrameCode, FrontierReason, OutcomeKind, Status},
    domain::relational::QueryReason,
    ssa,
};
use support::*;
use witness::Assignment;

#[test]
fn runtime_return_retries_the_whole_callback_entry() {
    let config = policy();
    let domain = domain(&config);
    fixture::entry_constructor_refusal(&domain);
    let code = fixture::recoverable();
    let analysis = run(&code, config);
    assert!(
        analysis.frontiers().is_empty(),
        "{:?}",
        analysis.frontiers()
    );
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.diagnostics().iter().any(|diagnostic| {
        diagnostic.kind == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
    }));
    let procedure = callback(&analysis, &code);
    let output = only_output(&procedure, OutcomeKind::Return);
    assert_eq!(
        output
            .store
            .read(address(B), &word(0), domain.borrowed())
            .singleton(),
        Some(U256::from(42))
    );
    assert_eq!(
        output
            .store
            .read_transient(address(B), &word(0), domain.borrowed())
            .singleton(),
        Some(U256::from(99))
    );
    let mut assignment = Assignment::default();
    assignment.exact_bytes(
        &output.data,
        &word(42).singleton().unwrap().to_be_bytes::<32>(),
    );
    assignment.store(&output.store, 42, 99, &domain);
    // Effects propagated to the unknown environment revisit this callback.
    // Counts bound closure without identifying the solver's private retry flag.
    assert!(procedure.input_updates > 0);
    bounded_current_evidence(&analysis);
    let graph = ssa::build_world(&analysis).unwrap();
    graph.verify(&analysis).unwrap();
    // The unknown envelope has no detailed callback instruction receipt.
    assert!(
        graph
            .blocks()
            .iter()
            .all(|block| block.instructions.is_empty())
    );
    for bit in [0, 1] {
        let observed = oracle::execute(&code, bit);
        observed.assert_callback(&code, OutcomeKind::Return, 42);
        let mut joint = Assignment::default();
        joint.exact_bytes(&output.data, &observed.callback_data);
        joint.store(&output.store, observed.slot, observed.transient, &domain);
    }
}

#[test]
fn runtime_revert_retries_with_rollback_and_exact_data() {
    let config = policy();
    let domain = domain(&config);
    fixture::entry_constructor_refusal(&domain);
    let code = fixture::reverting();
    let analysis = run(&code, config);
    assert!(
        analysis.frontiers().is_empty(),
        "{:?}",
        analysis.frontiers()
    );
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.diagnostics().iter().any(|diagnostic| {
        diagnostic.kind == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
    }));
    let procedure = callback(&analysis, &code);
    assert!(
        procedure
            .outputs
            .iter()
            .all(|output| output.kind != OutcomeKind::Return)
    );
    let output = only_output(&procedure, OutcomeKind::Revert);
    let mut assignment = Assignment::default();
    assignment.exact_bytes(
        &output.data,
        &word(99).singleton().unwrap().to_be_bytes::<32>(),
    );
    // Coarse saved roots may include more than the old singleton. One complete
    // valuation must still admit (7, 0); an output containing only (42, 99) fails.
    assignment.store(&output.store, 7, 0, &domain);
    bounded_current_evidence(&analysis);
    let graph = ssa::build_world(&analysis).unwrap();
    graph.verify(&analysis).unwrap();
    assert!(
        graph
            .blocks()
            .iter()
            .all(|block| block.instructions.is_empty())
    );
    for bit in [0, 1] {
        let observed = oracle::execute(&code, bit);
        observed.assert_callback(&code, OutcomeKind::Revert, 99);
        observed.assert_transient_writes();
        assert_ne!((observed.slot, observed.transient), (42, 99));
        let mut joint = Assignment::default();
        joint.exact_bytes(&output.data, &observed.callback_data);
        joint.store(&output.store, observed.slot, observed.transient, &domain);
    }
}

#[test]
fn runtime_body_growth_stays_incomplete_and_preserves_legal_revert() {
    let config = policy();
    let domain = domain(&config);
    fixture::body_constructor_refusal(&domain);
    let code = fixture::body_limited();
    let analysis = run(&code, config);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.work() < analysis.config().max_work);
    assert!(analysis.frontiers().iter().all(|frontier| {
        !matches!(
            frontier.reason,
            FrontierReason::Work | FrontierReason::SummaryWork | FrontierReason::Budget(_)
        )
    }));
    let procedure = callback(&analysis, &code);
    assert!(
        procedure
            .outputs
            .iter()
            .all(|output| output.kind != OutcomeKind::Return)
    );
    // This is B's own feasible CALLVALUE != 0 exit, not U's arbitrary revert.
    let output = only_output(&procedure, OutcomeKind::Revert);
    let mut joint = Assignment::default();
    joint.exact_bytes(
        &output.data,
        &word(99).singleton().unwrap().to_be_bytes::<32>(),
    );
    joint.store(&output.store, 7, 0, &domain);
    let refused: Vec<_> = analysis
        .frontiers()
        .iter()
        .filter(|frontier| {
            frontier.pc == Some(code.labels["refused"])
                && frontier.reason == FrontierReason::Relations(QueryReason::ExpressionLimit)
                && frontier.target.as_ref().is_some_and(|target| {
                    target.frames.last().is_some_and(|frame| {
                        frame.mode == FrameCode::Runtime
                            && frame.address == address(B)
                            && frame.code_address == address(B)
                            && frame.code_hash == procedure.frame.code_hash
                            && frame.basic_block_index == 2
                            && !frame.is_static
                    })
                })
        })
        .collect();
    assert!(!refused.is_empty(), "{:?}", analysis.frontiers());
    // Raw constructor frontiers in this baseline export their typed location,
    // but no Runtime payload or per-instruction receipt. Do not manufacture a
    // current/saved snapshot pair from the unknown root's unrelated state.
    for payload in refused
        .iter()
        .filter_map(|frontier| frontier.payload.as_deref())
    {
        assert!(
            payload.relations.is_empty(),
            "snapshot assignment requires the complete F to be True"
        );
        let mut snapshots = Assignment::default();
        snapshots.store(&payload.store, 99, 42, &domain);
        snapshots.store(payload.active().saved_store.state(), 7, 0, &domain);
    }
    bounded_current_evidence(&analysis);
    assert!(ssa::build_world(&analysis).is_err());
    let partial = ssa::build_partial_world(&analysis, &mut native::postprocessing()).unwrap();
    partial
        .verify(&analysis, &mut native::postprocessing())
        .unwrap();
    assert_eq!(partial.status(), Status::Incomplete);
    assert_eq!(partial.frontiers().len(), analysis.frontiers().len());
    for (retained, original) in partial.frontiers().iter().zip(analysis.frontiers()) {
        assert_eq!(retained.from, original.from);
        assert_eq!(retained.target, original.target);
        assert_eq!(retained.pc, original.pc);
        assert_eq!(retained.reason, original.reason);
        assert_eq!(retained.payload, original.payload);
    }
    assert!(
        partial
            .blocks()
            .iter()
            .all(|block| block.instructions.is_empty())
    );
    for bit in [0, 1] {
        let observed = oracle::execute(&code, bit);
        observed.assert_callback(&code, OutcomeKind::Return, 42);
        observed.assert_transient_writes();
    }
}
