use crate::domain::EvmNumericAlgebra as _;

use super::super::{Counters, StorageReadPolicy, Table, WorkBudget};
use crate::analysis::state::storage_observations::{StorageObservations, form_work};
#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::analysis::{
    ExecutionConfig,
    machine::fixtures::{fixture, word},
};
use crate::domain::symbolic::ExprId;
use crate::{analysis::MachinePayload, domain::ProductAlgebra};

fn written_input() -> (
    crate::analysis::testing::World,
    crate::analysis::testing::Entry,
    MachinePayload,
    crate::domain::ValueAlgebra<ProductAlgebra>,
) {
    let algebra = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let key = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key.clone()];
    let owner = input.active().key.address;
    input
        .store
        .write_checked(owner, &key, &word(7), domain)
        .unwrap();
    (world, entry, input, algebra)
}

#[test]
fn low_work_and_cancel_do_not_install_a_partial_summary() {
    let (world, entry, input, algebra) = written_input();
let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    let fee = form_work(&input, domain);
    println!(
        "STORAGE_OBSERVATION_FORM_FEE work={fee} store_work={}",
        input.store.work_size()
    );
    assert!(fee > 0);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(fee - 1);
    let mut counters = Counters::default();
    let result = table
        .solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
        .procedure(input.clone());
    assert_eq!(
        result.unwrap_err().frontier_reason(),
        crate::analysis::FrontierReason::Work
    );
    assert!(table.nodes.is_empty() && table.procedures.is_empty() && table.subscribers.is_empty());
    let control = crate::analysis::control::Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        assert!(StorageObservations::form(&input, domain).is_err());
        let mut budget = WorkBudget::new(config.max_work);
        let result = table
            .solver(
                &world,
                &entry,
                &config,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            )
            .procedure(input.clone());
        assert!(result.is_err());
        assert!(
            table.nodes.is_empty() && table.procedures.is_empty() && table.subscribers.is_empty()
        );
    });
}
