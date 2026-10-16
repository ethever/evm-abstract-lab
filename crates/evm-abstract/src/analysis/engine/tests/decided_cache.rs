use super::super::{Counters, run_metered};
use crate::{
    analysis::{
        ExecutionConfig, OutcomeKind, Status, machine::fixtures, transfer::StorageReadPolicy,
    },
    resource::WorkBudget,
    world::ByteArray,
};

#[test]
fn recursive_counter_retains_default_closure_under_local_reasoning() {
    let code = "610000351561002b576100016100003503610000525b6100006100006100206100006100006101015af1505b61002a61000052610020610000f3";
    let (world, mut entry) = fixtures::fixture(&[(0x101, code)]);
    entry.environment.calldata = ByteArray::unknown();
    let config = ExecutionConfig::default();
    let mut ledger = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let result = run_metered(
        world,
        entry,
        config,
        &mut ledger,
        &mut counters,
        StorageReadPolicy::Abstract,
    )
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| record.closed)
    );
    let returns = result
        .outcomes()
        .iter()
        .filter(|output| output.kind == OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert!(!returns.is_empty());
    for output in returns {
        assert_eq!(
            output
                .data
                .read_word(&fixtures::word(0), Default::default())
                .singleton(),
            Some(alloy_primitives::U256::from(42))
        );
    }
    assert!(ledger.used() > 0);
    assert!(!ledger.exhausted());
}
