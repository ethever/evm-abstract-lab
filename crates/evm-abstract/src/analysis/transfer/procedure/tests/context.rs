use crate::analysis::machine::fixtures::fixture;
use crate::domain::ProductAlgebra;

#[test]
fn context_depth_zero_stays_empty_during_actual_loop_execution_and_replay() {
    use super::super::{Counters, StorageReadPolicy, Table, WorkBudget};
    use crate::analysis::{ExecutionConfig, OutcomeKind};
    use alloy_primitives::U256;
    let (world, entry) = fixture(&[(0x101, "5f5b60010180601190106100015750602a5f5260205ff3")]);
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config.analysis.context_depth = 0;
    config_numeric.max_constants = 512;
    config_numeric.relations.enabled = false;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let mut table = Table::default();
    let mut counters = Counters::default();
    let mut budget = WorkBudget::new(config.max_work);
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty());
    assert!(record.closed);
    assert!(
        table
            .nodes
            .iter()
            .all(|node| node.payload.active().key.jump_history.is_empty())
    );
    assert!(table.nodes.len() <= 4);
    let returns = execution
        .outcomes
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert!(!returns.is_empty());
    for output in returns {
        assert_eq!(
            output
                .data
                .read_word(&crate::analysis::machine::fixtures::word(0), domain)
                .singleton(),
            Some(U256::from(42))
        );
    }
}
