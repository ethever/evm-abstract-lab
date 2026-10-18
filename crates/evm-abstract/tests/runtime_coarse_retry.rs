//! Real unknown-environment callbacks exercise the relative Runtime solver.
//! Internal retry flags and Runtime receipts remain the solver's private tests.

#[path = "runtime_coarse_retry/fixture.rs"]
mod fixture;
#[path = "runtime_coarse_retry/oracle.rs"]
mod oracle;
#[path = "runtime_coarse_retry/support.rs"]
mod support;
#[path = "runtime_coarse_retry/witness.rs"]
mod witness;

use evm_abstract::{
    analysis::{DiagnosticKind, OutcomeKind, Status},
    domain::relational::QueryReason,
    ssa,
};
use support::*;
use witness::Assignment;

#[test]
fn runtime_return_retries_the_whole_callback_entry() {
    let config = policy();
    let domain = config.domain().unwrap();
    fixture::entry_constructor_refusal(domain);
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
    let output = only_output(procedure, OutcomeKind::Return);
    assert_eq!(
        output.store.read(address(B), &word(0), domain).singleton(),
        Some(42.into())
    );
    assert_eq!(
        output
            .store
            .read_transient(address(B), &word(0), domain)
            .singleton(),
        Some(99.into())
    );
    let mut assignment = Assignment::default();
    assignment.exact_bytes(
        &output.data,
        &word(42).singleton().unwrap().to_be_bytes::<32>(),
    );
    assignment.store(&output.store, 42, 99, domain);
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
        joint.store(&output.store, observed.slot, observed.transient, domain);
    }
}
