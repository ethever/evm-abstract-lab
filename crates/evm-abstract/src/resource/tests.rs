use super::WorkBudget;
use analysis_control::{Cancellation, with_cancellation};

#[test]
fn optional_admission_is_read_only_and_tracks_cumulative_work() {
    let mut budget = WorkBudget::new(5);
    assert!(budget.can_afford(5));
    assert!(!budget.can_afford(6));
    assert!(!budget.can_afford(usize::MAX));
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
    assert!(budget.charge(3));
    for _ in 0..2 {
        assert!(budget.can_afford(0));
        assert!(budget.can_afford(2));
        assert!(!budget.can_afford(3));
        assert_eq!(budget.used(), 3);
        assert!(!budget.exhausted());
    }
    assert!(budget.charge(2));
    assert!(budget.can_afford(0));
    assert!(!budget.can_afford(1));
    assert_eq!(budget.used(), 5);
    assert!(!budget.exhausted());
}

#[test]
fn optional_admission_cannot_resurrect_a_failed_reservation() {
    let mut budget = WorkBudget::new(5);
    assert!(budget.charge(3));
    assert!(!budget.charge(3));
    for amount in [0, 1, 2] {
        assert!(!budget.can_afford(amount));
        assert_eq!(budget.used(), 3);
        assert!(budget.exhausted());
    }
}

#[test]
fn optional_admission_observes_cancellation_without_charging() {
    let token = Cancellation::new();
    let mut budget = with_cancellation(&token, || WorkBudget::new(5));
    assert!(budget.charge(2));
    assert!(budget.can_afford(3));
    token.cancel();
    for amount in [0, 1, 3] {
        assert!(!budget.can_afford(amount));
        assert_eq!(budget.used(), 2);
        assert!(budget.exhausted());
    }
    assert!(WorkBudget::new(5).can_afford(5));
}

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
