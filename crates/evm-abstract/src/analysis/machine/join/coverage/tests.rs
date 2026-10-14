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
use embedded_smt::{Cancellation, with_cancellation};
use revm_bytecode::opcode;

fn pair(rlimit: u32, equal: bool) -> (Domain, MachinePayload, MachinePayload, ExprId, ExprId) {
    let mut config = fixtures::config();
    config.analysis.relations.rlimit = rlimit;
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
            CheckResult::Sat
        );
    }
    (domain, previous, incoming, old, new)
}

#[test]
fn warm_coverage_preserves_the_original_paid_work_in_the_same_owner() {
    let (domain, previous, incoming, _, _) = pair(1_000_000, true);
    let before_previous = previous.clone();
    let before_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let mut cold_paid = None;
    for repeat in 0..2 {
        let before_work = budget.used();
        let before_cache = budget.smt_cache_stats();
        assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());
        let after_cache = budget.smt_cache_stats();
        let maintenance = (after_cache.maintenance_work - before_cache.maintenance_work) as usize;
        let paid = budget.used() - before_work - maintenance;
        assert!(paid >= domain.spec().relations().rlimit as usize);
        if let Some(cold) = cold_paid {
            assert_eq!(
                paid, cold,
                "a warm proof retains every original work charge"
            );
        } else {
            cold_paid = Some(paid);
        }
        assert_eq!(
            after_cache.candidate_hits - before_cache.candidate_hits,
            repeat
        );
        assert_eq!(previous, before_previous);
        assert_eq!(incoming, before_incoming);
        assert!(!budget.exhausted());
    }
    assert_eq!(budget.smt_cache_stats().entries, 1);
    let mut other = WorkBudget::new(100_000_000);
    assert!(covers(&previous, &incoming, domain, &mut other).unwrap());
    assert_eq!(other.smt_cache_stats().candidate_hits, 0);
}

#[test]
fn unknown_coverage_keeps_the_complete_incoming_alternative() {
    let (domain, previous, incoming, old, new) = pair(1, false);
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
        CheckResult::Unknown(QueryReason::ResourceLimit)
    );
    let before_previous = previous.clone();
    let before_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    assert!(!covers(&previous, &incoming, domain, &mut budget).unwrap());
    assert_eq!(budget.smt_cache_stats().undecided, 1);
    assert_eq!(budget.smt_cache_stats().entries, 0);
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
    assert_eq!(budget.smt_cache_stats().entries, 0);
    assert!(!budget.exhausted());
}

#[test]
fn guardless_and_cancelled_coverage_cannot_reuse_a_warm_proof() {
    let (domain, previous, incoming, _, _) = pair(1_000_000, true);
    let mut budget = WorkBudget::new(100_000_000);
    assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());
    let before_cache = budget.smt_cache_stats();
    let mut guardless = previous.clone();
    guardless.active_mut().stack[0].forget_expression();
    guardless.active_mut().stack[0].forget_identity();
    let before_guardless = guardless.clone();
    let before_incoming = incoming.clone();
    assert!(!covers(&guardless, &incoming, domain, &mut budget).unwrap());
    assert_eq!(budget.smt_cache_stats(), before_cache);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            covers(&previous, &incoming, domain, &mut budget),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!(budget.smt_cache_stats(), before_cache);
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
