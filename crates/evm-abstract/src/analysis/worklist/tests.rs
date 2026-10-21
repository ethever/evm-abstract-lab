//! Live priority scans preserve FIFO obligations and prepaid work on cancellation.

use super::{MachineError, MachineMeter, WorkBudget, kernel_failure};
use crate::domain::relational::QueryReason;
use analysis_control::{Cancellation, with_cancellation};
use analysis_kernel::worklist::Worklist;
use std::cell::RefCell;

const LIVE: [usize; 3] = [2, 0, 1];

fn live_worklists() -> [Worklist<usize>; 2] {
    let fresh = LIVE.into_iter().collect();
    let mut historical: Worklist<usize> = (3..10_003).chain(LIVE).collect();
    for id in 3..10_003 {
        historical.satisfy(id);
    }
    [fresh, historical]
}

#[test]
fn satisfied_history_does_not_affect_live_priority_scans_or_work() {
    let costs = live_worklists().map(|mut worklist| {
        let token = Cancellation::new();
        with_cancellation(&token, || {
            let mut budget = WorkBudget::new(usize::MAX);
            let visited = RefCell::new(Vec::new());
            assert_eq!(
                worklist.pop_deepest(&mut MachineMeter(&mut budget), |id| {
                    visited.borrow_mut().push(id);
                    1
                }),
                Ok(Some(LIVE[0]))
            );
            assert_eq!(visited.into_inner(), LIVE);
            let selection_work = budget.used();
            assert!(selection_work > 0);
            assert!(!budget.exhausted());

            // Equal priorities retain FIFO order even when IDs are unsorted.
            for id in LIVE.into_iter().skip(1) {
                assert_eq!(
                    worklist.pop_deepest(&mut MachineMeter(&mut budget), |_| 1),
                    Ok(Some(id))
                );
            }
            assert_eq!(
                worklist.pop_deepest(&mut MachineMeter(&mut budget), |_| 1),
                Ok(None)
            );
            assert!(worklist.pending().next().is_none());
            selection_work
        })
    });
    assert_eq!(costs[0], costs[1], "only live selection work is charged");
}

#[test]
fn cancellation_during_priority_scan_preserves_live_obligations_and_prepaid_work() {
    for before in live_worklists() {
        let mut control = before.clone();
        let control_token = Cancellation::new();
        let prepaid = with_cancellation(&control_token, || {
            let mut budget = WorkBudget::new(usize::MAX);
            assert_eq!(
                control.pop_deepest(&mut MachineMeter(&mut budget), |_| 1),
                Ok(Some(LIVE[0]))
            );
            assert!(budget.used() > 0);
            assert!(!budget.exhausted());
            budget.used()
        });

        // Exercise both the next-iteration checkpoint and the final checkpoint.
        for cancel_at in [0, LIVE.len() - 1] {
            let token = Cancellation::new();
            with_cancellation(&token, || {
                let mut worklist = before.clone();
                let mut budget = WorkBudget::new(usize::MAX);
                let visited = RefCell::new(Vec::new());
                let result = worklist.pop_deepest(&mut MachineMeter(&mut budget), |id| {
                    visited.borrow_mut().push(id);
                    if id == LIVE[cancel_at] {
                        token.cancel();
                    }
                    1
                });
                assert_eq!(
                    result.map_err(kernel_failure),
                    Err(MachineError::Relations(QueryReason::Cancelled))
                );
                assert_eq!(worklist, before);
                assert_eq!(worklist.pending().collect::<Vec<_>>(), [0, 1, 2]);
                assert_eq!(visited.into_inner(), LIVE[..=cancel_at]);
                assert_eq!(budget.used(), prepaid, "cancellation must not refund work");
                assert!(budget.exhausted());
            });
        }
    }
}
