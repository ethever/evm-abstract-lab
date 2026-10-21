use super::*;
use crate::{Availability, Change, Closure, KernelError, Nil, PublishError, solve};

#[test]
fn semantic_growth_invalidates_exact_transitive_adopters_and_withholds_stale_guarantees() {
    let mut f = Fixture::new();
    f.adopt();
    let routing_only = f.kernel.register(f.unrelated, (), &mut f.meter).unwrap();
    owner_read(&mut f.kernel, &mut f.meter, routing_only, &f.entry);
    f.kernel.retry(f.reader, &mut f.meter).unwrap();
    assert_eq!(f.kernel.next(&mut f.meter).unwrap(), Some(f.reader));
    let mut token = f.kernel.begin(f.reader, &mut f.meter).unwrap().suspend();
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        0,
        3,
    );
    assert!(f.stale(0));
    assert!(
        f.stale(1),
        "validity propagates even through a descending fact"
    );
    assert!(!f.stale(2), "routing identity is not a semantic assumption");
    assert!(f.kernel.worklist().contains(&f.owner));
    assert!(f.kernel.worklist().contains(&f.downstream_owner));
    assert!(f.kernel.worklist().contains(&routing_only));
    assert!(!f.kernel.worklist().contains(&f.reader));
    assert!(matches!(
        f.kernel.begin(f.reader, &mut f.meter),
        Err(KernelError::StaleGeneration)
    ));
    assert!(matches!(
        f.kernel.resume(&mut token, &mut f.meter),
        Err(KernelError::StaleGeneration)
    ));
    assert!(
        matches!(f.kernel.closure_in::<Stop>(f.target), Ok(Closure::Open(o)) if o.obligations == 1)
    );
    let len = f.local.len();
    let mut run = f.kernel.begin(routing_only, &mut f.meter).unwrap();
    assert_eq!(
        route(&mut Context::new(&mut run, &mut Nil), &f.entry)
            .unwrap()
            .state,
        Availability::Pending
    );
    assert_eq!(
        run.query(&mut f.local, Scoped::new(f.target, 0))
            .unwrap()
            .state,
        Availability::Pending
    );
    assert_eq!(
        run.keys(&mut f.local, f.target),
        Err(KernelError::StaleGeneration)
    );
    assert!(matches!(
        run.publish(&mut f.inputs, Scoped::new(f.target, 0), 7),
        Err(PublishError::Kernel(KernelError::StaleGeneration))
    ));
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(f.local.len(), len);
    struct NoOp;
    impl<Cx> Analyzer<Cx> for NoOp {
        type Task = ();
        type Error = Infallible;
        fn evaluate(&mut self, _: &(), _: &mut Cx) -> Result<LocalClosure, Infallible> {
            Ok(LocalClosure::Stable)
        }
    }
    let report = solve(&mut f.kernel, &mut NoOp, &mut Nil, &mut f.meter, 20).unwrap();
    assert_eq!(report.steps, 2, "only the two owners remain schedulable");
    assert!(matches!(report.closure, Closure::Open(o) if o.obligations == 2 && o.pending == 0));
}

#[test]
fn pending_absence_and_first_descending_value_only_wake_without_semantic_adoption() {
    let mut f = Fixture::new();
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    run.resolve_absent(&mut f.inputs, Scoped::new(f.source, 2))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query(&mut f.inputs, Scoped::new(f.source, 1))
            .unwrap()
            .state,
        Availability::Pending
    );
    assert_eq!(
        run.query(&mut f.inputs, Scoped::new(f.source, 2))
            .unwrap()
            .state,
        Availability::Absent
    );
    run.finish(LocalClosure::Stable).unwrap();
    for key in [1, 2] {
        publish(
            &mut f.kernel,
            &mut f.meter,
            f.writer,
            &mut f.inputs,
            f.source,
            key,
            1,
        );
        assert!(!f.stale(0));
        assert!(f.kernel.worklist().contains(&f.reader));
    }
    let mut first = f.kernel.channel(Descending(Meet), &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query(&mut first, Scoped::new(f.downstream, 0))
            .unwrap()
            .state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    seed(
        &mut f.kernel,
        &mut f.meter,
        f.downstream_reader,
        &mut first,
        3,
    );
    assert!(!f.stale(0));
    assert!(f.kernel.worklist().contains(&f.reader));
}

#[test]
fn new_semantic_topology_invalidates_its_directory_adopters_without_payload_growth() {
    let mut f = Fixture::new();
    let other = scope(&mut f.kernel, &mut f.meter, 5, 0);
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(run.keys(&mut f.inputs, f.source).unwrap(), [0]);
    run.finish(LocalClosure::Stable).unwrap();
    let other_reader = f.kernel.register(f.unrelated, (), &mut f.meter).unwrap();
    let mut run = f.kernel.begin(other_reader, &mut f.meter).unwrap();
    assert!(run.keys(&mut f.inputs, other).unwrap().is_empty());
    run.finish(LocalClosure::Stable).unwrap();
    // Equivalent existing payload publishes no new topology.
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        0,
        1,
    );
    assert!(!f.stale(0));
    // A new predecessor/key carries the very same value but changes membership.
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        1,
        1,
    );
    assert!(f.stale(0));
    assert!(!f.stale(2), "other scope's directory is independent");
    assert!(!f.kernel.worklist().contains(&other_reader));
}

#[test]
fn descending_refinement_wakes_external_consumers_without_reseeding_either_scope() {
    let mut f = Fixture::new();
    f.adopt();
    // The concrete allowed input is {0}; {0,1} -> {0} is a lawful precision
    // improvement. Never use an empty meet to model this nonempty universe.
    let mut coarse = f.kernel.channel(Descending(Meet), &mut f.meter).unwrap();
    seed(&mut f.kernel, &mut f.meter, f.reader, &mut coarse, 0b11);
    let mut run = f.kernel.begin(f.downstream_reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query(&mut coarse, Scoped::new(f.target, 0))
            .unwrap()
            .value(),
        Some(&0b11)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query(&mut coarse, Scoped::new(f.target, 0)).unwrap();
    refine(&mut run, &mut coarse, 0b01);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!f.stale(0) && !f.stale(1));
    assert!(f.kernel.worklist().contains(&f.reader));
    assert!(f.kernel.worklist().contains(&f.downstream_reader));
    assert!(!f.kernel.worklist().contains(&f.owner));
    assert!(!f.kernel.worklist().contains(&f.downstream_owner));
    for job in [f.reader, f.downstream_reader] {
        let mut run = f.kernel.begin(job, &mut f.meter).unwrap();
        assert_eq!(
            run.query(&mut coarse, Scoped::new(f.target, 0))
                .unwrap()
                .value(),
            Some(&0b01)
        );
        if job == f.reader {
            refine(&mut run, &mut coarse, 0b01);
        }
        run.finish(LocalClosure::Stable).unwrap();
    }
    assert_eq!(f.kernel.closure(), Closure::Closed);
    assert_eq!(f.entry, f.entry.clone());
    assert_ne!(f.entry, f.downstream_entry);
}

#[test]
fn fork_invalidation_is_isolated_and_commits_with_the_detached_ascending_value() {
    let mut f = Fixture::new();
    let slot = f.kernel.slot::<u8, _>(f.source, &mut f.meter).unwrap();
    let mut value = 1;
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.read_slot(&slot, &value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    for commit in [false, true] {
        let mut fork = f.kernel.fork(&mut f.meter).unwrap();
        let mut detached = value;
        let mut run = fork.kernel().begin(f.writer, &mut f.meter).unwrap();
        run.publish_prepared(&slot, &mut detached, 3, Change::Changed, 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(
            fork.view()
                .generation_stale(&f.entry, &mut f.meter)
                .unwrap()
        );
        assert!(!f.stale(0));
        assert_eq!(value, 1);
        if commit {
            f.kernel
                .commit_with(fork, &mut f.meter, 1, || value = detached)
                .unwrap();
            assert!(f.stale(0));
            assert_eq!(value, 3);
            assert!(f.kernel.worklist().contains(&f.owner));
        }
    }
}

#[test]
fn stale_admitted_jobs_are_obsolete_and_cannot_publish_or_hold_scheduler_open() {
    let mut f = Fixture::new();
    f.adopt();
    f.kernel.retry(f.reader, &mut f.meter).unwrap();
    assert_eq!(f.kernel.next(&mut f.meter).unwrap(), Some(f.reader));
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut f.inputs,
        f.source,
        0,
        3,
    );
    assert!(matches!(
        f.kernel.begin(f.reader, &mut f.meter),
        Err(KernelError::StaleGeneration)
    ));
    assert!(
        matches!(f.kernel.closure_in::<Stop>(f.target), Ok(Closure::Open(o)) if o.obligations == 1 && o.in_flight == 0)
    );
    assert_eq!(f.kernel.next(&mut f.meter).unwrap(), Some(f.owner));
}

#[test]
fn stale_selected_scope_read_withholds_sidecar_values_and_old_tasks_stay_unusable() {
    let mut f = Fixture::new();
    let meta = f.kernel.slot::<u8, _>(f.target, &mut f.meter).unwrap();
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
    assert_eq!(
        run.read_slot(&meta, &1, 1).unwrap().state,
        Availability::Pending
    );
    let old_entry = f.entry.clone();
    let next = {
        let (kernel, meter) = run.parts();
        let next = scope(kernel, meter, f.target.key().context, 1);
        kernel.seal_scope(next, meter).unwrap();
        next
    };
    run.replace_generation(&mut f.entry, next).unwrap();
    assert_eq!(f.entry, old_entry, "selector identity survives replacement");
    assert_eq!(
        run.current_generation(&old_entry).unwrap().value(),
        Some(&next)
    );
    assert_eq!(run.read_slot(&meta, &1, 1), Err(KernelError::InactiveScope));
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!f.stale(0));
    assert!(matches!(
        f.kernel.begin(f.reader, &mut f.meter),
        Err(KernelError::InvalidJob)
    ));
    f.meter.fail_on = Some(f.meter.calls + 1);
    assert_eq!(
        f.kernel.generation_stale(&f.entry, &mut f.meter),
        Err(KernelError::Stopped(Stop::Work))
    );
}

#[test]
fn refused_value_copy_records_a_wake_dependency_without_adopting_a_guarantee() {
    let mut f = Fixture::new();
    let slot = f.kernel.slot::<u8, _>(f.source, &mut f.meter).unwrap();
    let mut value = 1;
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.read_slot(&slot, &value, usize::MAX),
        Err(KernelError::CostOverflow)
    );
    drop(run);
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    run.publish_prepared(&slot, &mut value, 3, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!f.stale(0));
    assert!(f.kernel.worklist().contains(&f.reader));
}
