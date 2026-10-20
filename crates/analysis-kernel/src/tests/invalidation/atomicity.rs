use super::*;
use crate::{Change, KernelError, Meter, PublishError};
use std::cell::Cell;

fn publication_reservations() -> usize {
    let mut f = Fixture::new();
    f.adopt();
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    let start = run.meter().calls;
    run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3)
        .unwrap();
    run.meter().calls - start
}

#[test]
fn every_growth_reservation_refusal_preserves_value_versions_staleness_and_queue() {
    let reservations = publication_reservations();
    assert!(
        reservations >= 5,
        "exercise two-hop planning and final installation"
    );
    for fail_at in 1..=reservations {
        let mut f = Fixture::new();
        f.adopt();
        f.kernel.retry(f.reader, &mut f.meter).unwrap();
        let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
        let before = run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap();
        let entry_before = run.current_generation(&f.entry).unwrap();
        let queue_before = run.kernel().worklist().clone();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        assert_eq!(
            run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Work))),
            "reservation {fail_at}"
        );
        run.meter().fail_on = None;
        assert_eq!(
            run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap(),
            before
        );
        assert_eq!(run.current_generation(&f.entry).unwrap(), entry_before);
        assert_eq!(run.kernel().worklist(), &queue_before);
        run.finish(LocalClosure::Stable).unwrap();
        assert!(!f.stale(0));
        assert!(!f.stale(1));
        assert!(!f.kernel.worklist().contains(&f.owner));
    }
}

struct CancelAfter<'a> {
    root: &'a mut Budget,
    cancel_at: usize,
    cancelled: Cell<bool>,
}
impl Meter for CancelAfter<'_> {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.root.reserve(work)?;
        if self.root.calls == self.cancel_at {
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
fn cancellation_after_each_paid_growth_phase_including_last_reservation_is_atomic() {
    let reservations = publication_reservations();
    for cancel_after in 1..=reservations {
        let mut f = Fixture::new();
        f.adopt();
        f.kernel.retry(f.reader, &mut f.meter).unwrap();
        let before_queue = f.kernel.worklist().clone();
        let cancel_at = f.meter.calls + 1 + cancel_after; // begin, then publication phases
        let mut meter = CancelAfter {
            root: &mut f.meter,
            cancel_at,
            cancelled: Cell::new(false),
        };
        let mut run = f.kernel.begin(f.writer, &mut meter).unwrap();
        assert_eq!(
            run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Cancelled))),
            "phase {cancel_after}"
        );
        drop(run);
        assert_eq!(f.kernel.worklist(), &before_queue);
        assert!(!f.stale(0));
        assert!(!f.stale(1));
        let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
        assert_eq!(
            run.query(&mut f.inputs, Scoped::new(f.source, 0))
                .unwrap()
                .value(),
            Some(&1)
        );
        run.finish(LocalClosure::Stable).unwrap();
    }
}

#[test]
fn sidecar_install_callback_and_stale_scope_share_one_atomic_preflight() {
    for fail_at in 1..=5 {
        let mut f = Fixture::new();
        let slot = f.kernel.slot::<u8, _>(f.source, &mut f.meter).unwrap();
        let mut value = 1;
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.read_slot(&slot, &value, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
        let version = run.slot_version(&slot).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        let result = run.publish_prepared(&slot, &mut value, 3, Change::Changed, 1);
        run.meter().fail_on = None;
        match result {
            Err(KernelError::Stopped(Stop::Work)) => {
                assert_eq!(value, 1);
                assert_eq!(run.slot_version(&slot).unwrap(), version);
                run.finish(LocalClosure::Stable).unwrap();
                assert!(!f.stale(0));
                assert!(!f.kernel.worklist().contains(&f.owner));
            }
            Ok(Change::Changed) => {
                run.finish(LocalClosure::Stable).unwrap();
                assert_eq!(value, 3);
                assert!(f.stale(0));
                break;
            }
            other => panic!("unexpected sidecar result: {other:?}"),
        }
    }
}
