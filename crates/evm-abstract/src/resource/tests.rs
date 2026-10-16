use super::WorkBudget;
use analysis_control::{Cancellation, with_cancellation};

#[test]
fn reservations_are_transactional_and_do_not_refund_prior_work() {
    let mut budget = WorkBudget::new(5);
    assert!(budget.charge(3));
    assert!(!budget.charge(3));
    assert_eq!(budget.used(), 3);
    assert!(budget.exhausted());
}

#[test]
fn ledger_captures_one_way_cancellation_beyond_the_scope() {
    let token = Cancellation::new();
    let mut budget = with_cancellation(&token, || WorkBudget::new(10));
    assert!(budget.charge(2));
    token.cancel();
    assert!(!budget.charge(0));
    assert_eq!(budget.used(), 2);
    assert!(budget.exhausted());
    assert!(WorkBudget::new(10).charge(10));
}
