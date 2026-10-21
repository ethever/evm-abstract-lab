use super::support::{Ledger, Maximum, Numbers, Stop, scope};
use crate::analyzer::Noop;
use analysis_kernel::{
    Ascending, Availability, Change, Closure, Kernel, KernelError, LocalClosure, Nil, Scoped,
    SolveError, solve,
};

#[test]
fn zero_step_and_empty_queue_closure_reports_still_pay_and_observe_cancellation() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let used = meter.used;
    let report = solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0).unwrap();
    assert_eq!(report.closure, Closure::Closed);
    assert!(
        meter.used > used,
        "closure traversal cannot be a free report after work exhaustion"
    );
    meter.stop_after(1, Stop::Work);
    assert_eq!(
        solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0),
        Err(SolveError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    meter.cancelled.set(true);
    assert_eq!(
        solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0),
        Err(SolveError::Kernel(KernelError::Stopped(Stop::Cancelled)))
    );
    meter.cancelled.set(false);
    let pending = scope(&mut kernel, &mut meter, 84, 1);
    kernel.register(pending, (), &mut meter).unwrap();
    kernel.seal_scope(pending, &mut meter).unwrap();
    meter.stop_after(1, Stop::Work);
    assert_eq!(
        solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0),
        Err(SolveError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.pending == 1));
}

#[test]
fn earlier_sound_publication_survives_a_later_failure_in_the_same_job() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 30, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.publish(&mut values, Scoped::new(scope, 0), 7).unwrap();
    let retained = run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    run.query(&mut values, Scoped::new(scope, 1)).unwrap();
    run.meter().stop_after(1, Stop::Work);
    assert!(run.publish(&mut values, Scoped::new(scope, 1), 9).is_err());
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0)).unwrap(),
        retained
    );
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 1)).unwrap().state,
        Availability::Pending
    );
    drop(run);
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1 && open.missing == 1)
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StateAndEvidence {
    bound: u32,
    covered_inputs: Vec<u32>,
}

#[test]
fn prepared_value_metadata_and_version_are_atomic_before_notification_fanout() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 31, 0);
    let slot = kernel
        .slot::<StateAndEvidence, _>(scope, &mut meter)
        .unwrap();
    let mut current = StateAndEvidence {
        bound: 1,
        covered_inputs: vec![1],
    };
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    for _ in 0..2 {
        let reader = kernel.register(scope, (), &mut meter).unwrap();
        let mut run = kernel.begin(reader, &mut meter).unwrap();
        run.read_slot(&slot, &current, 8).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.read_slot(&slot, &current, 8).unwrap();
    run.meter().stop_after(1, Stop::Work);
    assert_eq!(
        run.publish_prepared(
            &slot,
            &mut current,
            StateAndEvidence {
                bound: 3,
                covered_inputs: vec![1, 2]
            },
            Change::Changed,
            10
        ),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(current, *before.value().unwrap());
    assert_eq!(run.slot_version(&slot).unwrap(), before.version);
    drop(run);
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1 && open.pending == 0)
    );
    kernel.retry(writer, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish_prepared(
        &slot,
        &mut current,
        StateAndEvidence {
            bound: 3,
            covered_inputs: vec![1, 2],
        },
        Change::Changed,
        10,
    )
    .unwrap();
    assert_ne!(run.slot_version(&slot).unwrap(), before.version);
    run.finish(LocalClosure::Stable).unwrap();
    let mut wakes = 0;
    while let Some(job) = kernel.next(&mut meter).unwrap() {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        assert_eq!(
            run.read_slot(&slot, &current, 8)
                .unwrap()
                .value()
                .unwrap()
                .covered_inputs,
            [1, 2]
        );
        run.finish(LocalClosure::Stable).unwrap();
        wakes += 1;
    }
    assert_eq!(wakes, 3, "two readers and the self-reading writer");
    assert_eq!(kernel.closure(), Closure::Closed);
}

fn fanout_cost(readers: usize) -> usize {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 32, 0);
    let slot = kernel.slot::<u32, _>(scope, &mut meter).unwrap();
    let mut current = 0;
    for _ in 0..readers {
        let job = kernel.register(scope, (), &mut meter).unwrap();
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.read_slot(&slot, &current, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.meter().used;
    run.publish_prepared(&slot, &mut current, 1, Change::Changed, 2)
        .unwrap();
    let cost = run.meter().used - before;
    run.finish(LocalClosure::Stable).unwrap();
    cost
}

#[test]
fn dependency_and_directory_work_are_metered_and_equal_reads_do_not_add_edges() {
    assert!(fanout_cost(8) > fanout_cost(1));
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 33, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    let after_first = run.kernel().dependency_count();
    let first_cost = run.meter().used;
    run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    assert_eq!(run.kernel().dependency_count(), after_first);
    assert!(run.meter().used > first_cost);
    run.query(&mut values, Scoped::new(scope, 1)).unwrap();
    assert!(run.kernel().dependency_count() > after_first);
    let before_keys = run.meter().used;
    assert!(run.keys(&mut values, scope).unwrap().is_empty());
    assert!(run.meter().used > before_keys);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.missing == 2));
}

#[test]
fn cancellation_during_read_keeps_the_job_unfinished_without_inventing_bottom() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 34, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = (run.meter().calls, run.meter().used);
    run.meter().cancelled.set(true);
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0)),
        Err(KernelError::Stopped(Stop::Cancelled))
    );
    assert_eq!(
        (run.meter().calls, run.meter().used),
        before,
        "pre-cancelled reads must not reserve work"
    );
    assert!(values.is_empty());
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
}
