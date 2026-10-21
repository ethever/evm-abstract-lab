use crate::domain::ProductAlgebra;
//! Real admission and interruption preserve FIFO, status partitions and bounds.

use super::*;
use crate::domain::EvmNumericAlgebra as _;

#[test]
fn duplicate_and_growing_inputs_park_each_resume_partition_with_its_current_bound() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut config = crate::analysis::machine::fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.use_summaries = true;
    config_numeric.relations.enabled = false;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut payload =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![word(7), word(1)];
    payload.normalize();
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let (root, success, failure);
    {
        let mut solver = table.solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        let procedure = solver.procedure(payload.clone()).unwrap();
        root = solver.table.procedures[procedure].first_body();
        success = solver
            .enqueue(procedure, payload.clone(), Some(true), root)
            .unwrap();
        let mut rejected = payload.clone();
        rejected.active_mut().stack[1] = word(0);
        failure = solver
            .enqueue(procedure, rejected, Some(false), root)
            .unwrap();
        let mut grown = payload.clone();
        grown.active_mut().stack[0] = word(9);
        for _ in 0..2 {
            assert_eq!(
                solver
                    .enqueue(procedure, grown.clone(), Some(true), root)
                    .unwrap(),
                success,
            );
        }
        assert_eq!(solver.table.nodes.len(), 3);
        assert_eq!(solver.table.nodes[success].updates, 1);
        assert_eq!(
            solver.runtime.pending_body().collect::<Vec<_>>(),
            [root, success, failure],
        );
    }
    counters.transfers = config.analysis.max_transfers;
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        payload,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(!record.closed);
    assert_eq!(record.transfers, 0);
    assert_eq!(execution.frontiers.len(), 3);
    assert_eq!(
        table.runtime.pending_body().collect::<Vec<_>>(),
        [success, failure],
    );
    let inputs = execution
        .frontiers
        .iter()
        .map(|frontier| {
            assert_eq!(frontier.reason, FrontierReason::Budget(Limit::Transfers));
            frontier.payload.as_ref().unwrap()
        })
        .collect::<Vec<_>>();
    // Distinct success/failure obligations have the same public location.
    assert_eq!(inputs[1].key(), inputs[2].key());
    assert_eq!(inputs[0].active().stack[0].singleton(), Some(U256::from(7)));
    assert!(inputs[1].active().stack[0].contains(U256::from(7)));
    assert!(inputs[1].active().stack[0].contains(U256::from(9)));
    assert!(!inputs[1].active().stack[0].contains(U256::from(8)));
    assert_eq!(inputs[1].active().stack[1].singleton(), Some(U256::ONE));
    assert_eq!(inputs[2].active().stack[1].singleton(), Some(U256::ZERO));
}
