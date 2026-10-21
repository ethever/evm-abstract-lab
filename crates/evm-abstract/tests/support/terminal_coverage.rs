//! Public work-stop oracle shared by symbolic-call and unknown-code fixtures.
use alloy_primitives::U256;
use evm_abstract::analysis::{
    EvmRootTerminalCoverage, EvmWorldAnalysis, ExecutionConfig, FrontierReason, OutcomeKind,
    RootTerminalScope, RootTerminalTrigger, Status,
};
use evm_abstract::domain::NumericValue;

/// Check the original request binding as one relation. Callers retain their
/// own program, budget, frontier-set and concrete-witness assertions.
pub fn assert_original_work_coverage<'a>(
    analysis: &'a EvmWorldAnalysis<NumericValue>,
    config: &ExecutionConfig,
) -> &'a EvmRootTerminalCoverage<NumericValue> {
    assert_eq!(analysis.status(), Status::Incomplete);
    assert_eq!(analysis.config().max_work, config.max_work);
    assert!(analysis.work() <= config.max_work);
    let coverage = analysis
        .root_terminal_coverage()
        .expect("the original ledger keeps the prepaid root terminal relation");
    let reason = match coverage.trigger() {
        RootTerminalTrigger::Work => FrontierReason::Work,
        RootTerminalTrigger::SummaryWork => FrontierReason::SummaryWork,
        other => panic!("expected an actual shared-ledger work stop: {other:?}"),
    };
    assert!(analysis.frontiers().iter().any(|f| f.reason == reason));
    assert_eq!(coverage.scope(), RootTerminalScope::AllInputStates);
    assert_eq!(
        coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
        [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure
        ]
    );
    let input = coverage.input();
    let original = &analysis.states()[0].entry;
    assert_eq!(input.entry(), analysis.entry());
    assert_eq!(input.frame(), &original.active().key);
    assert_eq!(input.program(), original.active().program.as_ref().unwrap());
    assert_eq!(input.calldata(), &original.active().calldata);
    assert_eq!(input.call_value(), &original.active().call_value);
    assert_eq!(input.saved_store(), &original.active().saved_store);
    assert_eq!(input.initial_store(), &original.store);
    assert_eq!(input.domain_spec(), analysis.domain_spec());
    assert_eq!(
        serde_json::to_value(input.config()).unwrap(),
        serde_json::to_value(config).unwrap()
    );
    assert_eq!(
        coverage.outcomes()[2].data().len().singleton(),
        Some(U256::ZERO)
    );
    coverage
}
