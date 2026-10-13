//! Evidence regressions for partial numerical procedure closure.

use super::*;
use crate::analysis::machine::fixtures::{address, fixture, word};
use alloy_primitives::U256;

#[test]
fn parked_joined_summary_contains_the_current_bound() {
    let domain = Domain::default();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = super::super::initial(&world, &entry, domain).unwrap();
    input
        .store
        .write(address(0x101), &word(0), &word(0), domain);
    let original = form(&input, &entry, domain).unwrap();
    input
        .store
        .write(address(0x101), &word(0), &word(1), domain);
    let incoming = form(&input, &entry, domain).unwrap();
    let joined = join(&original, &incoming, domain, false).unwrap();
    let mut table = Table::default();
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = Solver {
        table: &mut table,
        world: &world,
        entry: &entry,
        config: &config,
        domain,
        budget: &mut budget,
        counters: &mut counters,
        storage_reads: StorageReadPolicy::Abstract,
    };
    solver.frontier_summary(joined, FrontierReason::Budget(Limit::Transfers), 0);
    let frontier = &table.frontiers[0];
    assert_eq!(frontier.reason, FrontierReason::Budget(Limit::Transfers));
    let parked = frontier
        .payload
        .as_ref()
        .expect("current bound fits construction budget");
    let value = parked.store.read(address(0x101), &word(0), domain);
    assert!(value.contains(U256::ZERO));
    assert!(value.contains(U256::ONE));
    assert!(!value.contains(U256::from(2)));
}

#[test]
fn exhausted_ledger_does_not_replace_current_summary_with_an_old_input() {
    let domain = Domain::default();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let input = super::super::initial(&world, &entry, domain).unwrap();
    let summary = form(&input, &entry, domain).unwrap();
    let mut table = Table::default();
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(0);
    let mut counters = Counters::default();
    let mut solver = Solver {
        table: &mut table,
        world: &world,
        entry: &entry,
        config: &config,
        domain,
        budget: &mut budget,
        counters: &mut counters,
        storage_reads: StorageReadPolicy::Abstract,
    };
    solver.frontier_summary(summary, FrontierReason::Work, 0);
    assert!(table.frontiers[0].payload.is_none());
    assert_eq!(table.frontiers[0].target.as_ref(), Some(&input.key()));
    assert_eq!(table.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(budget.used(), 0);
}

#[test]
fn symbolic_owner_namespace_is_never_used_for_storage_discovery() {
    let domain = Domain::default();
    let (world, entry) = fixture(&[(0x101, "5f5400")]);
    let mut payload = super::super::initial(&world, &entry, domain).unwrap();
    let namespace = owner_namespace(&world, &payload.store);
    let frame = payload.active_mut();
    frame.key.address = namespace;
    frame.key.address_value = AddressInput::Concrete(namespace);
    frame.address_operand = Some(AbstractValue::unknown_address());
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let execution = execute_inner(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &mut budget,
        StorageReadPolicy::Discover,
        true,
    );
    assert!(
        !execution
            .frontiers
            .iter()
            .any(|frontier| matches!(frontier.reason, FrontierReason::MissingStorage { .. })),
        "synthetic namespace entered RPC discovery: {:?}",
        execution.frontiers
    );
    assert!(!execution.outcomes.is_empty());
}

#[test]
fn equal_exit_buffers_share_one_invocation_source_but_later_calls_are_fresh() {
    let (world, entry) = fixture(&[]);
    let config = crate::analysis::ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let payload = super::super::initial(&world, &entry, domain).unwrap();
    assert_eq!(payload.active().code, FrameCode::Unknown);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut call = || {
        table.execute(
            0,
            &world,
            &entry,
            payload.clone(),
            &config,
            domain,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
    };
    let (first, evidence) = call();
    assert!(evidence.closed);
    let returned = first
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Return)
        .unwrap();
    let reverted = first
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Revert)
        .unwrap();
    assert_eq!(
        returned.data.len().expression(),
        reverted.data.len().expression()
    );
    let first_length = returned.data.len().expression().unwrap().clone();
    let (second, evidence) = call();
    assert!(evidence.closed);
    let returned = second
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Return)
        .unwrap();
    assert_ne!(
        returned.data.len().expression(),
        Some(&first_length),
        "separate unknown calls must retain independent length choices"
    );
}
