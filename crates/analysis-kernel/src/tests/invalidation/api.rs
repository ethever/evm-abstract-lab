use super::*;
use crate::{Change, KernelError};

#[test]
fn metadata_observation_cumulatively_adopts_native_anchor_without_cloning_payload() {
    struct NonClone;
    let mut f = Fixture::new();
    let anchor = f
        .kernel
        .slot::<NonClone, _>(f.source, &mut f.meter)
        .unwrap();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    let snapshot_version = run.slot_version(&anchor).unwrap();
    assert_eq!(run.observe_slot(&anchor).unwrap(), snapshot_version);
    run.finish(LocalClosure::Stable).unwrap();
    // A later evaluation no longer reads the anchor; earlier narrowing still
    // depends on its old value and must not lose that generation-wide premise.
    for _ in 0..3 {
        f.kernel
            .begin(f.reader, &mut f.meter)
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
    }
    assert!(!f.kernel.scope_stale(f.target, &mut f.meter).unwrap());
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    run.publish_prepared_with(&anchor, Change::Changed, 1, || {})
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(f.kernel.scope_stale(f.target, &mut f.meter).unwrap());
    assert!(f.kernel.worklist().contains(&f.owner));
}

#[test]
fn stale_metadata_refuses_observation_and_scope_checks_preserve_meter_errors() {
    let mut f = Fixture::new();
    let local = f.kernel.slot::<u8, _>(f.target, &mut f.meter).unwrap();
    f.adopt();
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        0,
        3,
    );
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    assert_eq!(run.observe_slot(&local), Err(KernelError::StaleGeneration));
    run.finish(LocalClosure::Stable).unwrap();
    f.meter.fail_on = Some(f.meter.calls + 1);
    assert_eq!(
        f.kernel.scope_stale(f.target, &mut f.meter),
        Err(KernelError::Stopped(Stop::Work))
    );
    f.meter.fail_on = None;
    f.meter.cancelled.set(true);
    assert_eq!(
        f.kernel.scope_stale(f.target, &mut f.meter),
        Err(KernelError::Stopped(Stop::Cancelled))
    );
    f.meter.cancelled.set(false);
    let next = scope(&mut f.kernel, &mut f.meter, f.target.key().context, 1);
    f.kernel.seal_scope(next, &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    run.replace_generation(&mut f.entry, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        f.kernel.scope_stale(f.target, &mut f.meter).unwrap(),
        "retired callback is obsolete"
    );
    assert!(!f.kernel.scope_stale(next, &mut f.meter).unwrap());
    let mut other = Kernel::<()>::new().unwrap();
    let foreign = scope(&mut other, &mut f.meter, 0, 0);
    assert_eq!(
        f.kernel.scope_stale(foreign, &mut f.meter),
        Err(KernelError::WrongEpoch)
    );
}
