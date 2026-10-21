use super::*;
use crate::PublishError;

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
fn every_coverage_refresh_refusal_and_cancellation_keeps_guard_version_value_and_notifications() {
    let phases = renewal_reservations();
    assert!(phases > 0);
    for stop in [Stop::Work, Stop::Cancelled] {
        for phase in 1..=phases {
            let mut s = missing_coverage();
            let before = s.read_b();
            let queue = s.f.kernel.worklist().clone();
            // Resume with the same native premise for both failure modes.
            let mut token = {
                let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
                run.observe_slot(&s.anchor).unwrap();
                run.suspend()
            };
            let mut run = s.f.kernel.resume(&mut token, &mut s.f.meter).unwrap();
            run.meter().stop_after(phase, stop);
            assert_eq!(
                run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1),
                Err(PublishError::Kernel(KernelError::Stopped(stop))),
                "{stop:?} at reservation {phase}"
            );
            drop(run);
            s.f.meter.clear_stop();
            assert_eq!(s.f.kernel.worklist(), &queue);
            assert_eq!(s.read_b(), before);
            assert!(!s.f.stale(0) && !s.f.stale(1));
            assert_eq!(s.renew(), Change::Changed);
            let renewed = s.read_b();
            assert_eq!(renewed.value(), Some(&1));
            assert!(renewed.version > before.version);
        }
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
