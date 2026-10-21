use super::*;
use crate::Meter;
use std::cell::Cell;

fn withdrawal_phases() -> usize {
    let mut s = Scenario::new();
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    let before = run.meter().calls;
    run.withdraw_generation(&mut s.f.entry).unwrap();
    run.meter().calls - before
}

#[test]
fn every_withdrawal_refusal_preserves_selection_payload_handles_guards_and_wakes() {
    for fail_at in 1..=withdrawal_phases() {
        let mut s = Scenario::new();
        let bound = s.read_b();
        let queue = s.f.kernel.worklist().clone();
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        let before = run.current_generation(&s.f.entry).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        assert_eq!(
            run.withdraw_generation(&mut s.f.entry),
            Err(KernelError::Stopped(Stop::Work))
        );
        run.meter().fail_on = None;
        assert_eq!(run.current_generation(&s.f.entry).unwrap(), before);
        drop(run);
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert!(s.f.kernel.task::<Stop>(s.f.reader).is_ok());
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.read_b(), bound);
    }
}

struct CancelAfter<'a> {
    root: &'a mut Budget,
    at: usize,
    cancelled: Cell<bool>,
}
impl Meter for CancelAfter<'_> {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.root.reserve(work)?;
        if self.root.calls == self.at {
            self.cancelled.set(true);
        }
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled.get() {
            Err(Stop::Cancelled)
        } else {
            self.root.checkpoint()
        }
    }
}

#[test]
fn cancellation_after_every_withdrawal_payment_cannot_confirm_partial_absence() {
    for phase in 1..=withdrawal_phases() {
        let mut s = Scenario::new();
        let bound = s.read_b();
        let queue = s.f.kernel.worklist().clone();
        {
            let at = s.f.meter.calls + 1 + phase;
            let mut meter = CancelAfter {
                root: &mut s.f.meter,
                at,
                cancelled: Cell::new(false),
            };
            let mut run = s.f.kernel.begin(s.f.owner, &mut meter).unwrap();
            assert_eq!(
                run.withdraw_generation(&mut s.f.entry),
                Err(KernelError::Stopped(Stop::Cancelled))
            );
        }
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.read_b(), bound);
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        assert_eq!(
            run.current_generation(&s.f.entry).unwrap().value(),
            Some(&s.f.target)
        );
        run.finish(LocalClosure::Stable).unwrap();
    }
}

fn reactivation_phases() -> usize {
    let mut s = empty();
    let (next, _) = next_source(&mut s, 3);
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    let before = run.meter().calls;
    run.replace_generation(&mut s.f.entry, next).unwrap();
    run.meter().calls - before
}

#[test]
fn every_reactivation_refusal_preserves_empty_premises_and_can_retry_same_scope() {
    for phase in 1..=reactivation_phases() {
        let mut s = empty();
        let (next, _) = next_source(&mut s, 3);
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        let before = run.current_generation(&s.f.entry).unwrap();
        run.meter().fail_on = Some(run.meter().calls + phase);
        assert_eq!(
            run.replace_generation(&mut s.f.entry, next),
            Err(KernelError::Stopped(Stop::Work))
        );
        run.meter().fail_on = None;
        assert_eq!(run.current_generation(&s.f.entry).unwrap(), before);
        drop(run);
        assert!(!s.f.stale(1));
        assert_eq!(s.read_b().value(), Some(&1));
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        run.replace_generation(&mut s.f.entry, next).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(s.f.stale(1));
    }
}

#[test]
fn every_reactivation_cancellation_preserves_resolved_absence_and_fresh_consumers() {
    for phase in 1..=reactivation_phases() {
        let mut s = empty();
        let (next, _) = next_source(&mut s, 3);
        let queue = s.f.kernel.worklist().clone();
        {
            let at = s.f.meter.calls + 1 + phase;
            let mut meter = CancelAfter {
                root: &mut s.f.meter,
                at,
                cancelled: Cell::new(false),
            };
            let mut run = s.f.kernel.begin(s.f.owner, &mut meter).unwrap();
            assert_eq!(
                run.replace_generation(&mut s.f.entry, next),
                Err(KernelError::Stopped(Stop::Cancelled))
            );
        }
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.read_b().value(), Some(&1));
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        assert_eq!(
            run.current_generation(&s.f.entry).unwrap().state,
            Availability::Absent
        );
        run.finish(LocalClosure::Stable).unwrap();
    }
}
