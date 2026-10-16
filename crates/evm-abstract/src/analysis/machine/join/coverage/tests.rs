//! Coverage may discard an incoming alternative only after a complete proof.

use super::{MachineError, covers};
use crate::{
    analysis::{MachinePayload, machine::fixtures},
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, QueryReason},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use analysis_control::{Cancellation, with_cancellation};
use revm_bytecode::opcode;

fn pair(equal: bool) -> (Domain, MachinePayload, MachinePayload, ExprId, ExprId) {
    let config = fixtures::config();
    let domain = config.domain().unwrap();
    let old = ExprId::fresh().unwrap();
    let new = ExprId::fresh().unwrap();
    let mut previous = fixtures::payload(domain);
    previous.active_mut().stack = vec![AbstractValue::top().with_expression(old.clone())];
    let mut incoming = previous.clone();
    incoming.active_mut().stack = vec![AbstractValue::top().with_expression(new.clone())];
    if equal {
        assert_eq!(
            incoming
                .relations
                .add_equal(&old, &new, &domain.spec().relations()),
            CheckResult::MayBeFeasible
        );
    }
    (domain, previous, incoming, old, new)
}

#[test]
fn direct_coverage_proofs_pay_local_work_and_preserve_both_inputs() {
    let (domain, previous, incoming, _, _) = pair(true);
    let old_previous = previous.clone();
    let old_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    for _ in 0..2 {
        let before = budget.used();
        assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());
        assert!(budget.used() > before);
        assert_eq!(previous, old_previous);
        assert_eq!(incoming, old_incoming);
        assert!(!budget.exhausted());
    }
}

#[test]
fn unknown_coverage_keeps_the_complete_incoming_alternative() {
    let (domain, previous, incoming, old, new) = pair(false);
    let equality = ExprId::operation(
        opcode::EQ,
        &[old.clone(), new.clone()],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    assert_eq!(
        incoming
            .relations
            .can_equal(&equality, U256::ZERO, &domain.spec().relations()),
        CheckResult::MayBeFeasible
    );
    let before_previous = previous.clone();
    let before_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    assert!(!covers(&previous, &incoming, domain, &mut budget).unwrap());

    let mut alternatives = super::super::EntryAlternatives::new(&previous);
    let joined = alternatives
        .join(&previous, &incoming, domain, &mut budget)
        .unwrap()
        .expect("an undecided coverage check must retain the incoming alternative");
    let retained = joined.fresh_leaves();
    assert!(retained.contains(&old));
    assert!(retained.contains(&new));
    assert_eq!(previous, before_previous);
    assert_eq!(incoming, before_incoming);

    assert!(!budget.exhausted());
}

#[test]
fn guardless_and_cancelled_coverage_cannot_reuse_a_warm_proof() {
    let (domain, previous, incoming, _, _) = pair(true);
    let mut budget = WorkBudget::new(100_000_000);
    assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());

    let mut guardless = previous.clone();
    guardless.active_mut().stack[0].forget_expression();
    guardless.active_mut().stack[0].forget_identity();
    let before_guardless = guardless.clone();
    let before_incoming = incoming.clone();
    assert!(!covers(&guardless, &incoming, domain, &mut budget).unwrap());

    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            covers(&previous, &incoming, domain, &mut budget),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
    });

    assert_eq!(guardless, before_guardless);
    assert_eq!(incoming, before_incoming);
    let mut empty = WorkBudget::new(0);
    assert_eq!(
        covers(&previous, &incoming, domain, &mut empty),
        Err(MachineError::Work)
    );
    assert_eq!(empty.used(), 0);
    assert!(empty.exhausted());
}
