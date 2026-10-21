use super::*;
use crate::{Change, KernelError, PublishError};

fn publication_reservations() -> usize {
    let mut f = Fixture::new();
    f.adopt();
    f.kernel.retry(f.reader, &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap();
    run.current_generation(&f.entry).unwrap();
    let start = run.meter().calls;
    run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3)
        .unwrap();
    run.meter().calls - start
}

#[test]
fn every_growth_refusal_and_cancellation_preserves_value_versions_staleness_and_queue() {
    let reservations = publication_reservations();
    assert!(
        reservations >= 5,
        "exercise two-hop planning and final installation"
    );
    for stop in [Stop::Work, Stop::Cancelled] {
        for phase in 1..=reservations {
            let mut f = Fixture::new();
            f.adopt();
            f.kernel.retry(f.reader, &mut f.meter).unwrap();
            let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
            let before = run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap();
            let entry_before = run.current_generation(&f.entry).unwrap();
            let queue = run.kernel().worklist().clone();
            run.meter().stop_after(phase, stop);
            assert_eq!(
                run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3),
                Err(PublishError::Kernel(KernelError::Stopped(stop))),
                "{stop:?} at reservation {phase}"
            );
            run.meter().clear_stop();
            assert_eq!(
                run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap(),
                before
            );
            assert_eq!(run.current_generation(&f.entry).unwrap(), entry_before);
            drop(run);
            assert_eq!(f.kernel.worklist(), &queue);
            assert!(!f.stale(0) && !f.stale(1));
            assert!(!f.kernel.worklist().contains(&f.owner));
        }
    }
}

#[test]
fn sidecar_install_callback_and_stale_scope_share_one_atomic_preflight() {
    // The successful control quotes this exact fixture, so newly introduced
    // preflight payments are included in both fault sweeps automatically.
    let setup = || {
        let mut f = Fixture::new();
        let slot = f.kernel.slot::<u8, _>(f.source, &mut f.meter).unwrap();
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.read_slot(&slot, &1, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        (f, slot)
    };
    let phases = {
        let (mut f, slot) = setup();
        let mut value = 1;
        let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
        let version = run.slot_version(&slot).unwrap();
        let before = run.meter().calls;
        assert_eq!(
            run.publish_prepared(&slot, &mut value, 3, Change::Changed, 1)
                .unwrap(),
            Change::Changed
        );
        let phases = run.meter().calls - before;
        assert!(run.slot_version(&slot).unwrap() > version);
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(value, 3);
        assert!(f.stale(0));
        assert!(f.kernel.worklist().contains(&f.owner));
        assert!(phases > 0);
        phases
    };
    for stop in [Stop::Work, Stop::Cancelled] {
        for phase in 1..=phases {
            let (mut f, slot) = setup();
            let mut value = 1;
            let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
            let version = run.slot_version(&slot).unwrap();
            let queue = run.kernel().worklist().clone();
            run.meter().stop_after(phase, stop);
            assert_eq!(
                run.publish_prepared(&slot, &mut value, 3, Change::Changed, 1),
                Err(KernelError::Stopped(stop)),
                "{stop:?} at reservation {phase}"
            );
            run.meter().clear_stop();
            assert_eq!(value, 1);
            assert_eq!(run.slot_version(&slot).unwrap(), version);
            run.finish(LocalClosure::Stable).unwrap();
            assert_eq!(f.kernel.worklist(), &queue);
            assert!(!f.stale(0));
            assert!(!f.kernel.worklist().contains(&f.owner));
        }
    }
}
