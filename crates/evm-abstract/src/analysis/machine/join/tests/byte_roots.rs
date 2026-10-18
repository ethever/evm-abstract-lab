//! Rejected roots cannot absorb an alternative or prepare incoming content.

use super::{EntryAlternatives, MachineError, fixtures};
use crate::{
    domain::{relational::QueryReason, symbolic::ExprId},
    resource::WorkBudget,
    world::ByteArray,
};
use alloy_primitives::U256;
use analysis_control::{Cancellation, with_cancellation};

#[test]
fn later_matching_content_keeps_its_full_fee_after_root_mismatches() {
    let domain = fixtures::config().domain().unwrap();
    let mut entry = fixtures::payload(domain);
    entry.active_mut().stack = vec![fixtures::word(7)];
    entry.normalize();
    let mut incoming = entry.clone();
    incoming.active_mut().returndata = ByteArray::exact(&[7]);
    incoming.active_mut().stack[0] =
        fixtures::word(7).with_expression(ExprId::constant(U256::from(7)));
    let mut matching = incoming.clone();
    matching.active_mut().stack[0].forget_expression();
    assert_ne!(matching, incoming);
    assert_eq!(
        matching.content_form().unwrap(),
        incoming.content_form().unwrap()
    );
    let full_fee = matching
        .work_size()
        .saturating_add(incoming.work_size())
        .saturating_mul(4);
    let mut alternatives = EntryAlternatives::new(&entry);
    alternatives.retain(matching);
    let before = (
        entry.clone(),
        incoming.clone(),
        alternatives.represented.clone(),
    );
    // Entry and the first represented alternative both miss; only the later
    // matching candidate may prepare incoming content and spend its full fee.
    let mut budget = WorkBudget::new(15 + full_fee);
    assert!(
        alternatives
            .join(&entry, &incoming, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(budget.used(), 15 + full_fee);
    assert!(!budget.exhausted());
    assert_eq!((&entry, &incoming), (&before.0, &before.1));
    assert_eq!(alternatives.represented, before.2);
}

#[test]
fn matching_byte_roots_do_not_absorb_different_transaction_metadata() {
    let domain = fixtures::config().domain().unwrap();
    let entry = fixtures::payload(domain);
    let mut incoming = entry.clone();
    let owner = fixtures::address(0x101);
    incoming.store.write_balance(owner, fixtures::word(42));
    let before = (entry.clone(), incoming.clone());
    let mut alternatives = EntryAlternatives::new(&entry);
    let retained = alternatives.represented.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let joined = alternatives
        .join(&entry, &incoming, domain, &mut budget)
        .unwrap()
        .expect("root agreement cannot prove complete coverage");
    let balance = joined.store.read_balance(owner);
    assert!(balance.contains(U256::from(42)));
    assert!(balance.contains(entry.store.read_balance(owner).singleton().unwrap()));
    assert_eq!((&entry, &incoming), (&before.0, &before.1));
    assert_eq!(alternatives.represented, retained);
}

#[test]
fn root_rejected_join_still_pays_construction_and_preserves_failed_inputs() {
    let domain = fixtures::config().domain().unwrap();
    let entry = fixtures::payload(domain);
    let mut incoming = entry.clone();
    incoming.active_mut().returndata = ByteArray::exact(&[7]);
    let before = (entry.clone(), incoming.clone());
    let mut alternatives = EntryAlternatives::new(&entry);
    let retained = alternatives.represented.clone();
    let mut unpaid = WorkBudget::new(0);
    assert_eq!(
        alternatives.join(&entry, &incoming, domain, &mut unpaid),
        Err(MachineError::Work)
    );
    assert_eq!(unpaid.used(), 0);
    let mut rejected = WorkBudget::new(10);
    assert_eq!(
        alternatives.join(&entry, &incoming, domain, &mut rejected),
        Err(MachineError::Work)
    );
    assert_eq!(rejected.used(), 10);
    assert!(rejected.exhausted());
    let mut warm = WorkBudget::new(100_000_000);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            alternatives.join(&entry, &incoming, domain, &mut warm),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!((&entry, &incoming), (&before.0, &before.1));
    assert_eq!(alternatives.represented, retained);
}
