use super::*;
use crate::{
    analysis::{
        machine::fixtures::{fixture, word},
        transfer,
    },
    domain::{ProductAlgebra, ValueAlgebra, symbolic::ExprId},
};

#[test]
fn full_copy_equality_and_replay_keep_aliases_and_observations() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let key = EvmValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key.clone(), key.clone()];
    input
        .store
        .write_checked(input.active().key.address, &key, &word(7), domain)
        .unwrap();
    let original = super::super::form(&input, &entry, domain).unwrap();
    assert_ne!(original.storage_observations, Default::default());
    let mut budget = WorkBudget::new(1_000_000_000);
    let paid = form_normalized_with_payment(&input, &entry, domain, &mut budget).unwrap();
    assert!(
        paid.eq_with_payment(&original, domain, &mut budget)
            .unwrap()
    );
    let copy = paid.clone_with_payment(&mut budget).unwrap();
    assert_eq!(copy.storage_observations, original.storage_observations);
    assert!(copy.active().stack_aliases.same_class(0, 1));
    let mut missing = copy.clone();
    missing.storage_observations = Default::default();
    assert!(!copy.eq_with_payment(&missing, domain, &mut budget).unwrap());
    let first = instantiate_with_payment(&copy, domain, &mut budget).unwrap();
    let second = instantiate_with_payment(&copy, domain, &mut budget).unwrap();
    assert_eq!(
        first.active().stack[0].expression(),
        first.active().stack[1].expression()
    );
    assert_ne!(
        first.active().stack[0].expression(),
        second.active().stack[0].expression()
    );
    assert_eq!(first.relations.len(), 1);
    let restored = form_normalized_with_payment(&first, &entry, domain, &mut budget).unwrap();
    assert_eq!(restored.storage_observations, copy.storage_observations);
}

#[test]
fn independent_copy_equality_and_replay_refuse_their_own_ledger() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let input = transfer::initial(&world, &entry, domain).unwrap();
    let summary = super::super::form(&input, &entry, domain).unwrap();
    assert!(matches!(
        summary.clone_with_payment(&mut WorkBudget::new(0)),
        Err(MachineError::Work)
    ));
    assert!(matches!(
        summary.eq_with_payment(&summary, domain, &mut WorkBudget::new(0)),
        Err(MachineError::Work)
    ));
    assert!(matches!(
        instantiate_with_payment(&summary, domain, &mut WorkBudget::new(0)),
        Err(MachineError::Work)
    ));
    let mut exact = WorkBudget::new(1_000_000_000);
    summary.clone_with_payment(&mut exact).unwrap();
    let fee = exact.used();
    assert!(matches!(
        summary.clone_with_payment(&mut WorkBudget::new(fee - 1)),
        Err(MachineError::Work)
    ));
    assert!(
        summary
            .clone_with_payment(&mut WorkBudget::new(fee))
            .is_ok()
    );
}
