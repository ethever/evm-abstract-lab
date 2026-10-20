use super::*;
use crate::{Meter, PublishError};
use std::cell::Cell;

fn missing_coverage() -> Scenario {
    let mut s = Scenario::new();
    s.grow();
    s.replace(false);
    s.replace(true);
    s.drain_owners();
    assert_eq!(s.read_b().state, Availability::Pending);
    s
}
fn renewal_reservations() -> usize {
    let mut s = missing_coverage();
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    run.observe_slot(&s.anchor).unwrap();
    let before = run.meter().calls;
    run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1)
        .unwrap();
    run.meter().calls - before
}

#[test]
fn every_coverage_refresh_refusal_keeps_old_guard_version_value_and_notifications() {
    for fail_at in 1..=renewal_reservations() {
        let mut s = missing_coverage();
        let before = s.read_b();
        let queue = s.f.kernel.worklist().clone();
        let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
        run.observe_slot(&s.anchor).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        assert_eq!(
            run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
        );
        drop(run);
        s.f.meter.fail_on = None;
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert_eq!(s.read_b(), before);
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.renew(), Change::Changed);
        assert_eq!(s.read_b().value(), Some(&1));
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
fn cancellation_at_every_refresh_phase_including_final_payment_is_atomic() {
    for cancel_after in 1..=renewal_reservations() {
        let mut s = missing_coverage();
        let before = s.read_b();
        let queue = s.f.kernel.worklist().clone();
        // Attach the native premise before the operation whose phases we count.
        let mut token = {
            let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
            run.observe_slot(&s.anchor).unwrap();
            run.suspend()
        };
        let at = s.f.meter.calls + 1 + cancel_after; // resume reservation
        let mut meter = CancelAfter {
            root: &mut s.f.meter,
            at,
            cancelled: Cell::new(false),
        };
        let mut run = s.f.kernel.resume(&mut token, &mut meter).unwrap();
        assert_eq!(
            run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Cancelled)))
        );
        drop(run);
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert_eq!(s.read_b(), before);
        assert!(!s.f.stale(1));
    }
}

fn third_family(s: &mut Scenario) -> (JobId, SlotMeta<u8>) {
    let c =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    let anchor = s.f.kernel.slot(s.f.source, &mut s.f.meter).unwrap();
    let mut run = s.f.kernel.begin(c, &mut s.f.meter).unwrap();
    run.observe_slot(&anchor).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    (c, anchor)
}

#[test]
fn every_new_family_refusal_keeps_old_coverage_and_invalidation_atomic() {
    let reservations = {
        let mut s = Scenario::new();
        let (c, _) = third_family(&mut s);
        let mut run = s.f.kernel.begin(c, &mut s.f.meter).unwrap();
        let before = run.meter().calls;
        run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1)
            .unwrap();
        run.meter().calls - before
    };
    for fail_at in 1..=reservations {
        let mut s = Scenario::new();
        let before = s.read_b();
        let (c, anchor) = third_family(&mut s);
        let queue = s.f.kernel.worklist().clone();
        let mut run = s.f.kernel.begin(c, &mut s.f.meter).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        assert_eq!(
            run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
        );
        drop(run);
        s.f.meter.fail_on = None;
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert_eq!(s.read_b(), before);
        assert!(!s.f.stale(1));
        let mut value = 1;
        let mut run = s.f.kernel.begin(s.f.writer, &mut s.f.meter).unwrap();
        run.publish_prepared(&anchor, &mut value, 3, Change::Changed, 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(s.f.stale(2));
        assert!(
            !s.f.stale(1),
            "a refused C declaration must not become B's premise"
        );
        assert_eq!(s.read_b().value(), Some(&1));
    }
}

#[test]
fn native_anchor_growth_refusal_preserves_the_automatically_derived_invalidation_wave() {
    let reservations = {
        let mut s = Scenario::new();
        let mut run = s.f.kernel.begin(s.f.writer, &mut s.f.meter).unwrap();
        let before = run.meter().calls;
        run.publish_prepared(&s.anchor, &mut s.native, 3, Change::Changed, 1)
            .unwrap();
        run.meter().calls - before
    };
    for fail_at in 1..=reservations {
        let mut s = Scenario::new();
        let before = s.read_b();
        let queue = s.f.kernel.worklist().clone();
        let mut run = s.f.kernel.begin(s.f.writer, &mut s.f.meter).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        assert_eq!(
            run.publish_prepared(&s.anchor, &mut s.native, 3, Change::Changed, 1),
            Err(KernelError::Stopped(Stop::Work))
        );
        drop(run);
        s.f.meter.fail_on = None;
        assert_eq!(s.native, 1);
        assert_eq!(s.f.kernel.worklist(), &queue);
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.read_b(), before);
    }
}
