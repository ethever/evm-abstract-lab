//! A cancellation after finding a candidate must preserve all open obligations.

use super::{MachineError, WorkBudget, Worklist};
use crate::domain::relational::QueryReason;

#[test]
fn cancellation_during_priority_scan_keeps_candidates_and_stale_records_atomic() {
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut worklist: Worklist = [0, 1, 2].into_iter().collect();
        worklist.satisfy(0);
        let before = worklist.clone();
        let mut budget = WorkBudget::new(100);
        let result = worklist.pop_deepest(&mut budget, |id| {
            if id == 1 {
                // There is already a live deepest candidate; a later checkpoint
                // must not consume it or drop the other pending input.
                token.cancel();
            }
            1
        });
        assert_eq!(result, Err(MachineError::Relations(QueryReason::Cancelled)));
        assert_eq!(worklist, before);
        assert_eq!(worklist.pending().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(budget.used(), 7);
        assert!(budget.exhausted());
    });
}
