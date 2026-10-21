use super::{Budget, Stop, scope};
use crate::{Change, Closure, Kernel, KernelError, LocalClosure};

#[test]
fn suspended_outer_consumer_and_nested_jobs_share_real_dependencies() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let outer_scope = scope(&mut kernel, &mut meter, 1, 0);
    let body_scope = scope(&mut kernel, &mut meter, 2, 0);
    let outer = kernel.register(outer_scope, "outer", &mut meter).unwrap();
    let body = kernel.register(body_scope, "body", &mut meter).unwrap();
    let output_meta = kernel.slot(body_scope, &mut meter).unwrap();
    let mut output = 0;
    kernel.seal_scope(outer_scope, &mut meter).unwrap();
    kernel.seal_scope(body_scope, &mut meter).unwrap();
    let mut run = kernel.begin(outer, &mut meter).unwrap();
    assert_eq!(
        run.read_slot(&output_meta, &output, 1).unwrap().value(),
        Some(&0)
    );
    let mut token = run.suspend();
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.in_flight == 1));
    assert!(matches!(
        kernel.begin(outer, &mut meter),
        Err(KernelError::AlreadyRunning)
    ));
    assert_eq!(
        kernel
            .next_deepest(&mut meter, |task| usize::from(*task == "body"))
            .unwrap(),
        Some(body)
    );
    let mut run = kernel.begin(body, &mut meter).unwrap();
    run.publish_prepared(&output_meta, &mut output, 7, Change::Changed, 2)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        kernel.closure_in::<Stop>(body_scope).unwrap(),
        Closure::Closed
    );
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.in_flight == 1));
    assert!(kernel.worklist().contains(&outer));
    assert_eq!(
        kernel.next(&mut meter).unwrap(),
        None,
        "a dirty suspended job is not reentered"
    );
    let mut run = kernel.resume(&mut token, &mut meter).unwrap();
    assert_eq!(
        run.read_slot(&output_meta, &output, 1).unwrap().value(),
        Some(&7)
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(
        kernel.resume(&mut token, &mut meter),
        Err(KernelError::NotRunning)
    ));
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(outer));
    kernel
        .begin(outer, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
    assert_eq!(kernel.statistics().evaluations, 3);
}

#[test]
fn abandoning_a_token_and_failed_resume_never_fabricate_closure() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let job = kernel.register(s, (), &mut meter).unwrap();
    kernel.seal_scope(s, &mut meter).unwrap();
    let mut token = kernel.begin(job, &mut meter).unwrap().suspend();
    meter.cancelled.set(true);
    assert!(matches!(
        kernel.resume(&mut token, &mut meter),
        Err(KernelError::Stopped(Stop::Cancelled))
    ));
    meter.cancelled.set(false);
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.in_flight == 1));
    drop(kernel.resume(&mut token, &mut meter).unwrap());
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.interrupted == 1));
    kernel.retry(job, &mut meter).unwrap();
    let new_token = kernel.begin(job, &mut meter).unwrap().suspend();
    assert!(matches!(
        kernel.resume(&mut token, &mut meter),
        Err(KernelError::NotRunning)
    ));
    let _abandoned = new_token;
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.in_flight == 1));
}

#[test]
fn notifications_received_after_admission_survive_begin_and_finish() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let reader = kernel.register(s, (), &mut meter).unwrap();
    let writer = kernel.register(s, (), &mut meter).unwrap();
    let meta = kernel.slot(s, &mut meter).unwrap();
    let mut value = 0;
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    run.read_slot(&meta, &value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel.retry(reader, &mut meter).unwrap();
    kernel
        .begin(writer, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(reader));
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish_prepared(&meta, &mut value, 1, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel
        .begin(reader, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(reader));
}
