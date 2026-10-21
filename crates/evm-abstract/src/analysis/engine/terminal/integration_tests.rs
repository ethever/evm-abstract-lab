//! Real engine boundaries keep detailed evidence separate from root coverage.
use crate::domain::ProductAlgebra;

#[cfg(test)]
use crate::analysis::testing::{Entry, World};

use crate::{
    U256,
    analysis::{
        ExecutionConfig, FrontierReason, InstructionProgress, Limit, OutcomeKind,
        RootTerminalScope, RootTerminalTrigger, Status, WorldAnalysis, control::Control,
        machine::fixtures,
    },
    resource::WorkBudget,
    ssa,
};

const TWO_BLOCK_RETURN: &str = "6003565b602a5f5260205ff3";

fn run(
    world: &World,
    entry: &Entry,
    config: ExecutionConfig,
    spec: crate::domain::DomainSpec,
) -> WorldAnalysis {
    crate::analysis::testing::execute(world.clone(), entry.clone(), config, spec).unwrap()
}

fn partial_ssa(analysis: &WorldAnalysis) -> ssa::PartialWorldSsa<crate::domain::NumericValue> {
    Control::default().scope(|| {
        let mut postprocessing = WorkBudget::new(2_000_000_000);
        let partial = ssa::build_partial_world(analysis, &mut postprocessing).unwrap();
        partial.verify(analysis, &mut postprocessing).unwrap();
        partial
    })
}

fn assert_same_detail(expected: &WorldAnalysis, actual: &WorldAnalysis) {
    assert_eq!(actual.status(), expected.status());
    assert_eq!(actual.transfers(), expected.transfers());
    assert_eq!(actual.states().len(), expected.states().len());
    for (after, before) in actual.states().iter().zip(expected.states()) {
        assert_eq!(after.id, before.id);
        assert_eq!(after.key, before.key);
        assert_eq!(after.executed_pcs, before.executed_pcs);
        match (after.execution_evidence(), before.execution_evidence()) {
            (Some(after), Some(before)) => {
                assert_eq!(after.is_current(), before.is_current());
                assert_eq!(after.instructions(), before.instructions());
                assert_eq!(after.instruction_values(), before.instruction_values());
                assert_eq!(after.successors(), before.successors());
            }
            (None, None) => {}
            _ => panic!("coverage preparation changed detailed execution receipts"),
        }
    }
    assert_eq!(actual.frontiers().len(), expected.frontiers().len());
    for (after, before) in actual.frontiers().iter().zip(expected.frontiers()) {
        assert_eq!(after.from, before.from);
        assert_eq!(after.pc, before.pc);
        assert_eq!(after.target, before.target);
        assert_eq!(after.reason, before.reason);
    }
    assert_eq!(actual.outcomes().len(), expected.outcomes().len());
    for (after, before) in actual.outcomes().iter().zip(expected.outcomes()) {
        assert_eq!(after.state, before.state);
        assert_eq!(after.kind, before.kind);
        assert_eq!(after.data, before.data);
        assert_eq!(after.store, before.store);
    }
}

#[test]
fn explicitly_disabled_keeps_existing_detail_and_native_omission() {
    let (world, entry) = fixtures::fixture(&[(0x101, TWO_BLOCK_RETURN)]);
    let mut config = fixtures::config();
    let config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.analysis.max_transfers = 1;
    config.root_terminal_coverage = false;
    let ordinary = run(&world, &entry, config.clone(), config_numeric.spec());
    config.root_terminal_coverage = false;
    let explicitly_disabled = run(&world, &entry, config, config_numeric.spec());
    assert_same_detail(&ordinary, &explicitly_disabled);
    assert_eq!(ordinary.work(), explicitly_disabled.work());
    assert_eq!(ordinary.status(), Status::Incomplete);
    assert!(ordinary.root_terminal_coverage().is_none());
    assert!(
        serde_json::to_value(&ordinary)
            .unwrap()
            .get("root_terminal_coverage")
            .is_none()
    );
}

#[test]
fn default_on_keeps_state_and_transfer_caps_separate_from_terminal_coverage() {
    let (world, entry) = fixtures::fixture(&[(0x101, TWO_BLOCK_RETURN)]);
    for (max_states, max_transfers, limit, trigger) in [
        (1, 1024, Limit::States, RootTerminalTrigger::States),
        (256, 1, Limit::Transfers, RootTerminalTrigger::Transfers),
    ] {
        let mut config = ExecutionConfig::default();
        let config_numeric = crate::analysis::testing::NumericLimits::default();
        assert!(config.root_terminal_coverage);
        assert_eq!(config.max_work, 20_000_000);
        config.analysis.max_states = max_states;
        config.analysis.max_transfers = max_transfers;
        let mut disabled = config.clone();
        disabled.root_terminal_coverage = false;
        let ordinary = run(&world, &entry, disabled, config_numeric.spec());
        let covered = run(&world, &entry, config.clone(), config_numeric.spec());
        assert_same_detail(&ordinary, &covered);
        assert_eq!(covered.status(), Status::Incomplete);
        assert_eq!(covered.transfers(), 1);
        assert_eq!(covered.frontiers().len(), 1);
        assert_eq!(covered.frontiers()[0].reason, FrontierReason::Budget(limit));
        assert_eq!(covered.states()[0].executed_pcs, [0, 2]);
        assert!(covered.states()[0].execution_evidence().is_some());
        assert_eq!(covered.outcomes().len(), 1);
        assert_eq!(covered.outcomes()[0].kind, OutcomeKind::Failure);

        let coverage = covered.root_terminal_coverage().unwrap();
        assert_eq!(coverage.scope(), RootTerminalScope::AllInputStates);
        assert_eq!(coverage.trigger(), &trigger);
        assert_eq!(
            coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
            [
                OutcomeKind::Return,
                OutcomeKind::Revert,
                OutcomeKind::Failure
            ]
        );
        assert_eq!(coverage.input().entry(), &entry);
        assert_eq!(coverage.input().frame(), covered.states()[0].active());
        assert_eq!(
            serde_json::to_value(coverage.input().config()).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
        let partial = partial_ssa(&covered);
        assert_eq!(partial.status(), Status::Incomplete);
        assert_eq!(partial.frontiers().len(), covered.frontiers().len());
        assert_eq!(partial.blocks().len(), covered.states().len());
        assert_eq!(partial.blocks()[0].instructions.len(), 2);
        assert!(ssa::build_world(&covered).is_err());
    }
}

#[test]
fn actual_state_and_transfer_caps_attach_only_separate_terminal_coverage() {
    let (world, entry) = fixtures::fixture(&[(0x101, TWO_BLOCK_RETURN)]);
    for (max_states, max_transfers, limit, trigger) in [
        (1, 1024, Limit::States, RootTerminalTrigger::States),
        (256, 1, Limit::Transfers, RootTerminalTrigger::Transfers),
    ] {
        let mut config = fixtures::config();
        let config_numeric = crate::analysis::testing::NumericLimits::machine();
        config.max_work = usize::MAX;
        config.analysis.max_states = max_states;
        config.analysis.max_transfers = max_transfers;
        config.root_terminal_coverage = false;
        let ordinary = run(&world, &entry, config.clone(), config_numeric.spec());
        config.root_terminal_coverage = true;
        let covered = run(&world, &entry, config, config_numeric.spec());
        assert_same_detail(&ordinary, &covered);
        assert_eq!(covered.status(), Status::Incomplete);
        assert_eq!(covered.transfers(), 1);
        assert!(
            covered
                .frontiers()
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Budget(limit))
        );
        assert!(covered.states()[0].execution_evidence().is_some());
        assert_eq!(covered.states()[0].executed_pcs, [0, 2]);
        assert_eq!(covered.outcomes().len(), 1);
        assert_eq!(covered.outcomes()[0].kind, OutcomeKind::Failure);

        let coverage = covered.root_terminal_coverage().unwrap();
        assert_eq!(coverage.scope(), RootTerminalScope::AllInputStates);
        assert_eq!(coverage.trigger(), &trigger);
        assert_eq!(
            coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
            [
                OutcomeKind::Return,
                OutcomeKind::Revert,
                OutcomeKind::Failure
            ]
        );
        let partial = partial_ssa(&covered);
        assert_eq!(partial.status(), Status::Incomplete);
        assert_eq!(partial.frontiers().len(), covered.frontiers().len());
        assert_eq!(partial.blocks().len(), covered.states().len());
        assert_eq!(partial.blocks()[0].instructions.len(), 2);
        assert!(ssa::build_world(&covered).is_err());

        let native = serde_json::to_value(&covered).unwrap();
        assert_eq!(
            native["status"],
            serde_json::to_value(Status::Incomplete).unwrap()
        );
        assert_eq!(
            native["frontiers"].as_array().unwrap().len(),
            covered.frontiers().len()
        );
        assert_eq!(
            native["outcomes"].as_array().unwrap().len(),
            covered.outcomes().len()
        );
        assert_eq!(
            native["root_terminal_coverage"]["outcomes"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}

#[test]
fn actual_work_refusal_keeps_the_original_pending_mload_and_partial_ssa() {
    // The first budget admitting MLOAD's visit cannot pay its positive read fee.
    // This is the existing partial-SSA prefix, with all cutoffs measured here.
    let (world, entry) = fixtures::fixture(&[(0x101, "602a60405100")]);
    let mut config = fixtures::config();
    let config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.max_work = usize::MAX;
    config.root_terminal_coverage = false;
    let complete = run(&world, &entry, config.clone(), config_numeric.spec());
    assert_eq!(complete.status(), Status::Converged);
    config.root_terminal_coverage = true;
    let prepared_complete = run(&world, &entry, config.clone(), config_numeric.spec());
    assert_same_detail(&complete, &prepared_complete);
    assert!(prepared_complete.root_terminal_coverage().is_none());
    let preparation = prepared_complete
        .work()
        .checked_sub(complete.work())
        .unwrap();
    assert!(preparation > 0);

    config.root_terminal_coverage = false;
    let (mut minimum, mut maximum) = (1, complete.work());
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        config.max_work = midpoint;
        let candidate = run(&world, &entry, config.clone(), config_numeric.spec());
        if candidate
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&4))
        {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    config.max_work = minimum;
    let ordinary = run(&world, &entry, config.clone(), config_numeric.spec());
    config.root_terminal_coverage = true;
    config.max_work = minimum.checked_add(preparation).unwrap();
    let covered = run(&world, &entry, config, config_numeric.spec());
    assert_same_detail(&ordinary, &covered);
    assert_eq!(covered.status(), Status::Incomplete);
    assert_eq!(covered.states()[0].executed_pcs, [0, 2, 4]);
    assert_eq!(covered.outcomes().len(), 1);
    assert_eq!(covered.outcomes()[0].kind, OutcomeKind::Failure);
    assert_eq!(
        covered.work().checked_sub(ordinary.work()),
        Some(preparation)
    );
    assert!(covered.frontiers().iter().any(|frontier| {
        frontier.from == Some(0)
            && frontier.pc == Some(4)
            && frontier.reason == FrontierReason::Work
    }));
    assert_eq!(
        covered.root_terminal_coverage().unwrap().trigger(),
        &RootTerminalTrigger::Work
    );
    let partial = partial_ssa(&covered);
    assert_eq!(partial.status(), Status::Incomplete);
    assert_eq!(partial.blocks()[0].instructions.len(), 3);
    assert_eq!(
        partial.blocks()[0].instructions[2].progress,
        InstructionProgress::OperandsConsumed
    );
    assert_eq!(partial.frontiers().len(), covered.frontiers().len());
    assert!(ssa::build_world(&covered).is_err());
}

#[test]
fn exact_return_42_retains_its_strict_detailed_outcomes_without_coverage() {
    let (world, entry) = fixtures::fixture(&[(0x101, "602a5f5260205ff3")]);
    let mut config = fixtures::config();
    let config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.max_work = usize::MAX;
    config.root_terminal_coverage = false;
    let ordinary = run(&world, &entry, config.clone(), config_numeric.spec());
    config.root_terminal_coverage = true;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let covered = run(&world, &entry, config, config_numeric.spec());
    assert_same_detail(&ordinary, &covered);
    assert_eq!(covered.status(), Status::Converged);
    assert!(covered.frontiers().is_empty());
    assert!(covered.root_terminal_coverage().is_none());
    let returns: Vec<_> = covered
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0].data.len().singleton(), Some(U256::from(32)));
    assert_eq!(
        returns[0]
            .data
            .read_word(&fixtures::word(0), domain)
            .singleton(),
        Some(U256::from(42))
    );
    ssa::build_world(&covered)
        .unwrap()
        .verify(&covered)
        .unwrap();
}
