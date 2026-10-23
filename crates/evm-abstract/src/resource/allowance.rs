//! Optional phases lower the original ledger's ceiling; they never mint work.

use super::WorkBudget;

impl WorkBudget {
    /// Permanently lower remaining work on the SAME ledger across session/RPC
    /// rounds. Accepted work and the declared maximum stay unchanged. Repeated
    /// calls cannot enlarge the ceiling, refund work or revive sticky refusal.
    /// Saturation only clamps a limit; it never establishes a payable invoice.
    pub(crate) fn limit_remaining(&mut self, allowance: usize) -> Result<(), super::CloneError> {
        super::clone_payment::checkpoint(self)?;
        self.ceiling = self
            .ceiling
            .min(self.maximum)
            .min(self.consumed.saturating_add(allowance));
        super::clone_payment::checkpoint(self)
    }

    /// Unspent admitted work under both the root and persistent phase ceilings.
    pub(crate) fn remaining(&self) -> usize {
        if self.exhausted() {
            0
        } else {
            self.maximum.min(self.ceiling).saturating_sub(self.consumed)
        }
    }

    /// Spend at most `allowance` additional units on this same ledger. Refusal
    /// stays sticky after the parent ceiling is restored; there is no retry or
    /// refund. Callers must retain already certified results separately.
    pub(crate) fn with_allowance<T>(
        &mut self,
        allowance: usize,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> Option<T> {
        if !self.can_afford(0) {
            return None;
        }
        let parent = self.maximum;
        self.maximum = parent.min(self.consumed.saturating_add(allowance));
        let result = operation(self);
        self.maximum = parent;
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::WorkBudget;
    use crate::resource::CloneError;

    #[test]
    fn one_way_limit_preserves_spent_declared_maximum_and_sticky_refusal() {
        let mut budget = WorkBudget::new(100);
        assert!(budget.charge(7));
        budget.limit_remaining(20).unwrap();
        assert_eq!(
            (budget.maximum, budget.used(), budget.remaining()),
            (100, 7, 20)
        );
        budget.limit_remaining(90).unwrap();
        assert_eq!(budget.remaining(), 20);
        assert!(budget.charge(10));
        budget.limit_remaining(90).unwrap();
        assert_eq!(budget.remaining(), 10);
        assert!(budget.charge(10));
        assert!(!budget.charge(1));
        assert_eq!(budget.limit_remaining(100), Err(CloneError::Work));
        assert_eq!(
            (budget.maximum, budget.used(), budget.ceiling),
            (100, 27, 27)
        );
        assert!(!budget.can_afford(0));
        assert!(budget.with_allowance(100, |_| unreachable!()).is_none());
    }

    #[test]
    fn one_way_limit_survives_temporary_allowance_and_saturates_without_new_work() {
        let mut budget = WorkBudget::new(100);
        assert!(budget.charge(7));
        budget.limit_remaining(usize::MAX).unwrap();
        assert_eq!(budget.remaining(), 93);
        assert_eq!(
            budget.with_allowance(5, |b| {
                b.limit_remaining(3).unwrap();
                assert!(b.charge(2));
                b.remaining()
            }),
            Some(1)
        );
        assert_eq!(
            (budget.maximum, budget.used(), budget.remaining()),
            (100, 9, 1)
        );
        budget.limit_remaining(usize::MAX).unwrap();
        assert_eq!(budget.remaining(), 1);
    }

    #[test]
    fn one_way_limit_checks_original_and_ambient_cancellation_without_payment() {
        let original = analysis_control::Cancellation::new();
        let mut budget = analysis_control::with_cancellation(&original, || WorkBudget::new(100));
        assert!(budget.charge(7));
        let ambient = analysis_control::Cancellation::new();
        ambient.cancel();
        let refused = analysis_control::with_cancellation(&ambient, || budget.limit_remaining(10));
        assert_eq!(refused, Err(CloneError::Cancelled));
        assert_eq!(
            (budget.maximum, budget.ceiling, budget.used()),
            (100, 100, 7)
        );
        budget.limit_remaining(10).unwrap();
        original.cancel();
        assert_eq!(budget.limit_remaining(1), Err(CloneError::Cancelled));
        assert_eq!(
            (budget.maximum, budget.ceiling, budget.used()),
            (100, 17, 7)
        );
    }

    #[test]
    fn zero_remaining_limit_is_not_a_payment_or_a_false_work_refusal() {
        let mut budget = WorkBudget::new(100);
        assert!(budget.charge(7));
        budget.limit_remaining(0).unwrap();
        assert_eq!((budget.used(), budget.remaining()), (7, 0));
        assert!(!budget.exhausted());
        assert!(budget.can_afford(0));
        assert!(!budget.charge(1));
        assert!(budget.exhausted());
    }

    #[test]
    fn allowance_preserves_actual_spent_and_sticky_refusal() {
        let mut budget = WorkBudget::new(100);
        assert!(budget.charge(7));
        assert_eq!(
            budget.with_allowance(12, |ledger| {
                assert_eq!(ledger.used(), 7);
                assert!(ledger.charge(9));
                assert!(!ledger.charge(4));
                ledger.used()
            }),
            Some(16)
        );
        assert_eq!(budget.used(), 16);
        assert!(budget.exhausted());
        assert!(!budget.can_afford(0));
        assert!(!budget.can_afford(1));
        assert!(budget.with_allowance(84, |_| unreachable!()).is_none());
    }

    #[test]
    fn allowance_does_not_prepay_unused_work_or_exceed_parent() {
        let mut budget = WorkBudget::new(20);
        assert!(budget.charge(5));
        assert_eq!(budget.with_allowance(10, |b| b.charge(4)), Some(true));
        assert_eq!(budget.used(), 9);
        assert!(budget.can_afford(11));
        assert_eq!(budget.with_allowance(100, |b| b.charge(12)), Some(false));
        assert_eq!(budget.used(), 9);
        assert!(budget.exhausted());
    }
}
