//! Optional phases lower the original ledger's ceiling; they never mint work.

use super::WorkBudget;

impl WorkBudget {
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
