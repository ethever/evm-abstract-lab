//! Complete public-API client for a finite saturating counter language.
//!
//! States, residue may-sets and terminal accumulators ascend. Joins have finite
//! height (at most eight interval expansions and two residue-bit additions per
//! point); each scoped point is admitted once. No abstract worklist or outer
//! fixed-point loop exists here: `solve` alone closes the coupled transfers.
//! Temporary interval/residue intersections are monotone in both growing inputs.
//! Semantic coverage is asserted at joint closure, not for partial frontiers.
//! This finite witness does not establish universal client soundness.
#[path = "interpreter/driver.rs"]
mod driver;
#[path = "interpreter/intervals.rs"]
mod intervals;
#[path = "interpreter/language.rs"]
mod language;
#[path = "interpreter/parity.rs"]
mod parity;
#[path = "interpreter/reference.rs"]
mod reference;

use analysis_kernel::{Availability, Closure, KernelError, Reachable, Scoped, SolveError, solve};
use driver::Harness;
use language::{ContextId, Interval, PROGRAM, Pc, Stop};

fn range(lo: u8, hi: u8) -> language::State {
    Reachable::Value(Interval { lo, hi })
}

#[test]
fn client_interpreter_closes_branches_and_loops_with_two_feedback_removal_controls() {
    for reverse in [false, true] {
        for (parities, intervals) in [(true, true), (false, true), (true, false)] {
            let mut h = Harness::new(parities, intervals, reverse);
            // Shared routes and identical PCs, distinct client context identities.
            let inputs = [0b0000_0101, 0b0000_0010, 0xff, 0];
            let scopes =
                std::array::from_fn::<_, 4, _>(|i| h.invocation(ContextId(i as u64), inputs[i]));
            let report = h.solve(512);
            assert_eq!(report.closure, Closure::Closed);
            assert!(!report.local_limits);
            assert!(report.steps < 512);
            let snapshots = scopes.map(|scope| h.snapshot(scope));
            for (snapshot, words) in snapshots.iter().zip(inputs) {
                reference::covers(snapshot, PROGRAM, words, parities);
            }
            let [even, one, unknown, empty] = snapshots;
            assert_eq!(one.returned, Reachable::Bottom);
            assert_eq!(one.faulted, range(1, 1));
            assert_eq!(unknown.states[Pc::Entry as usize], range(0, 7));
            assert_eq!(unknown.parities[Pc::Entry as usize], 0b11);
            assert_eq!(unknown.returned, range(6, 7));
            assert_eq!(unknown.faulted, range(1, 1));
            assert!(empty.states.iter().all(|s| *s == Reachable::Bottom));
            assert_eq!(
                (empty.returned, empty.faulted),
                (Reachable::Bottom, Reachable::Bottom)
            );
            if parities && intervals {
                assert_eq!(even.returned, range(6, 6));
                assert_eq!(even.faulted, Reachable::Bottom);
                assert_eq!(even.parities[Pc::Head as usize], 0b01);
                assert!(
                    !h.analyzers
                        .right
                        .left
                        .admitted
                        .contains(&Scoped::new(scopes[0], Pc::Fault))
                );
            } else {
                // Either removed direction loses a real terminal precision gain.
                assert_eq!(even.returned, range(6, 7));
                if !parities {
                    assert_eq!(even.faulted, range(1, 1));
                    assert!(h.analyzers.right.right.trace.is_empty());
                } else {
                    // Without interval feedback, parity cannot exclude saturating
                    // 6 + 2 -> 7 at Step; its loop invariant grows to Top.
                    assert_eq!(even.parities[Pc::Head as usize], 0b11);
                    assert_eq!(even.faulted, Reachable::Bottom);
                }
            }
            assert_eq!(h.solve(512).steps, 0, "repeated joins must stabilize");
        }
    }
}

#[test]
fn late_initial_facts_wake_executed_consumers_and_loop_growth_wakes_them_again() {
    let mut h = Harness::new(true, true, false);
    let scope = h.invocation(ContextId(20), 0b0000_0101);
    let first = h.solve(2);
    assert!(matches!(first.closure, Closure::Open(o) if o.missing == 2 && o.pending == 1));
    let entry = Scoped::new(scope, Pc::Entry);
    assert_eq!(
        h.analyzers.right.left.trace,
        [(entry.clone(), Availability::Pending)]
    );
    assert_eq!(
        h.analyzers.right.right.trace,
        [(entry.clone(), Availability::Pending)]
    );
    assert_eq!(h.kernel.statistics().jobs, 3);

    let seeded = h.solve(1);
    assert!(matches!(seeded.closure, Closure::Open(o) if o.missing == 0 && o.pending == 2));
    // No retry or new consumer registration: the two old readers were awakened.
    assert_eq!(h.kernel.statistics().jobs, 3);
    h.meter.cancelled = true;
    let stopped = solve(
        &mut h.kernel,
        &mut h.analyzers,
        &mut h.stores,
        &mut h.meter,
        512,
    );
    assert!(matches!(
        stopped,
        Err(SolveError::Kernel(KernelError::Stopped(Stop::Cancelled)))
    ));
    assert_eq!(h.kernel.closure(), seeded.closure);
    h.meter.cancelled = false;

    assert_eq!(h.solve(512).closure, Closure::Closed);
    let snapshot = h.snapshot(scope);
    reference::covers(&snapshot, PROGRAM, 0b0000_0101, true);
    assert_eq!(snapshot.returned, range(6, 6));
    assert_eq!(snapshot.faulted, Reachable::Bottom);
    for trace in [
        &h.analyzers.right.left.trace,
        &h.analyzers.right.right.trace,
    ] {
        assert_eq!(trace.iter().filter(|(key, _)| *key == entry).count(), 2);
        let head = Scoped::new(scope, Pc::Head);
        assert!(trace.contains(&(head.clone(), Availability::Value(range(0, 2)))));
        assert!(trace.contains(&(head, Availability::Value(range(0, 6)))));
    }
    assert!(
        h.kernel.statistics().jobs > 3,
        "successors must be registered during execution"
    );
    assert!(h.kernel.statistics().wake_attempts > 2);
    assert!(h.meter.used > 0);
    assert_eq!(h.solve(512).steps, 0);
}
