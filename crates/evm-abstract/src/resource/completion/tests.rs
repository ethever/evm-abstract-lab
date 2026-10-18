use super::{AdmissionStop, CompletionPermit, WorkBudget};
use analysis_control::{Cancellation, with_cancellation};

const FEE: usize = 7;

#[test]
fn exact_bound_is_fully_prepaid_and_publication_does_not_charge() {
    let mut budget = WorkBudget::new(FEE);
    let permit = budget.try_completion_escrow(FEE).unwrap();
    assert_eq!(budget.used(), FEE);
    assert!(!budget.exhausted());

    assert!(permit.publish_allowed());
    assert_eq!(budget.used(), FEE);
    assert!(!budget.exhausted());
}

#[test]
fn one_less_than_bound_declines_without_poisoning_or_refunding_discovery() {
    const DISCOVERY: usize = 3;
    let mut budget = WorkBudget::new(DISCOVERY + FEE - 1);
    assert!(budget.charge(DISCOVERY));
    assert_eq!(
        budget.try_completion_escrow(FEE).unwrap_err(),
        AdmissionStop::Insufficient
    );
    assert_eq!(budget.used(), DISCOVERY);
    assert!(!budget.exhausted());
    assert!(budget.charge(FEE - 1));
    assert_eq!(budget.used(), DISCOVERY + FEE - 1);
    assert!(!budget.exhausted());
}

#[test]
fn overflow_sentinel_declines_even_when_the_ledger_maximum_would_fit() {
    let mut budget = WorkBudget::new(usize::MAX);
    assert_eq!(
        budget.try_completion_escrow(usize::MAX).unwrap_err(),
        AdmissionStop::Overflow
    );
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
    assert!(budget.charge(FEE));
}

#[test]
fn sticky_exhaustion_rejects_escrow_even_when_a_later_charge_succeeds() {
    let mut budget = WorkBudget::new(FEE + 1);
    assert!(budget.charge(2));
    assert!(!budget.charge(FEE));
    assert!(budget.charge(1));
    assert_eq!(budget.used(), 3);
    for bound in [0, 1, FEE, usize::MAX] {
        assert_eq!(
            budget.try_completion_escrow(bound).unwrap_err(),
            AdmissionStop::AlreadyExhausted
        );
        assert_eq!(budget.used(), 3);
        assert!(budget.exhausted());
    }
}

#[test]
fn cancellation_accessor_keeps_the_original_payer_across_scopes() {
    let payer = Cancellation::new();
    let unrelated = Cancellation::new();
    let mut budget = with_cancellation(&payer, || WorkBudget::new(FEE));
    let captured = with_cancellation(&unrelated, || budget.cancellation());
    unrelated.cancel();
    assert!(!captured.is_cancelled());

    captured.cancel();
    assert!(payer.is_cancelled());
    assert_eq!(
        budget.try_completion_escrow(FEE).unwrap_err(),
        AdmissionStop::Cancelled
    );
    assert_eq!(budget.used(), 0);
    assert!(budget.exhausted());
}

#[test]
fn permit_uses_the_payer_cancellation_even_outside_its_scope() {
    let payer = Cancellation::new();
    let unrelated = Cancellation::new();
    let mut budget = with_cancellation(&payer, || WorkBudget::new(2 * FEE));
    let first = with_cancellation(&unrelated, || budget.try_completion_escrow(FEE)).unwrap();
    unrelated.cancel();
    assert!(with_cancellation(&unrelated, || first.publish_allowed()));

    let second = with_cancellation(&unrelated, || budget.try_completion_escrow(FEE)).unwrap();
    payer.cancel();
    assert!(!second.publish_allowed());
    assert_eq!(budget.used(), 2 * FEE);
    assert!(budget.exhausted());
}

#[test]
fn moved_permit_publishes_after_exhaustion_without_mutating_the_ledger() {
    fn publish(permit: CompletionPermit) -> bool {
        permit.publish_allowed()
    }

    let mut budget = WorkBudget::new(FEE + 2);
    let permit = budget.try_completion_escrow(FEE).unwrap();
    assert!(budget.charge(1));
    assert!(!budget.charge(2));
    assert_eq!(budget.used(), FEE + 1);
    assert!(budget.exhausted());

    // 按值移交和消费；许可没有 Clone，也不借用仍可继续记录工作的账本。
    assert!(publish(permit));
    assert_eq!(budget.used(), FEE + 1);
    assert!(budget.exhausted());
}

#[test]
fn discarding_prepaid_results_never_refunds_the_ledger() {
    let mut budget = WorkBudget::new(FEE);
    let permit = budget.try_completion_escrow(FEE).unwrap();
    drop(permit);
    assert_eq!(budget.used(), FEE);
    assert!(!budget.exhausted());
    assert!(!budget.can_afford(1));
}
