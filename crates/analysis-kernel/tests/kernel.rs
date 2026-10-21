//! Behavioral contracts for typed cooperative solving and production adapter primitives.

#[path = "kernel/cooperation.rs"]
mod cooperation;
#[path = "kernel/publication_drop.rs"]
mod publication_drop;
#[path = "kernel/reclaim.rs"]
mod reclaim;
#[path = "kernel/retirement.rs"]
mod retirement;
#[path = "kernel/support.rs"]
mod support;
#[path = "kernel/worklist.rs"]
mod worklist;

use analysis_kernel::{
    Ascending, Availability, Change, Closure, Kernel, KernelError, LocalClosure, Meter,
    PublishError, Scoped,
};
use support::{Budget, Maximum, Numbers, Stop, scope};

#[test]
fn missing_and_directory_reads_wake_on_late_provider_first_output() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let reader = kernel.register(scope, "reader", &mut meter).unwrap();
    let directory = kernel.register(scope, "directory", &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(directory, &mut meter).unwrap();
    assert!(run.keys(&mut channel, scope).unwrap().is_empty());
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 7))
            .unwrap()
            .state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!kernel.worklist().contains(&directory));
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.missing == 1 && open.pending == 0)
    );
    let provider = kernel.register(scope, "provider", &mut meter).unwrap();
    kernel
        .provide(&mut channel, Scoped::new(scope, 7), provider, &mut meter)
        .unwrap();
    let mut run = kernel.begin(provider, &mut meter).unwrap();
    assert_eq!(
        run.publish(&mut channel, Scoped::new(scope, 7), 42)
            .unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&reader));
    assert!(kernel.worklist().contains(&directory));
    let admitted = std::array::from_fn::<_, 2, _>(|_| kernel.next(&mut meter).unwrap().unwrap());
    assert!(admitted.contains(&reader) && admitted.contains(&directory));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 7))
            .unwrap()
            .value(),
        Some(&42)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(directory, &mut meter).unwrap();
    assert_eq!(run.keys(&mut channel, scope).unwrap(), [7]);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn failed_publication_keeps_value_version_and_wakes_atomic_but_retains_interruption() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let reader = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(scope, 0), 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.query(&mut channel, Scoped::new(scope, 0)).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let used_before = run.meter().used;
    run.meter().stop_after(3, Stop::Work); // lookup, policy, install+wakes
    assert_eq!(
        run.publish(&mut channel, Scoped::new(scope, 0), 2),
        Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    drop(run);
    assert!(
        meter.used > used_before,
        "successful earlier reservations are not refunded"
    );
    assert!(!kernel.worklist().contains(&reader));
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
    meter.clear_stop();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 0)).unwrap(),
        before
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.retry(writer, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(scope, 0), 2).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&reader));
}

#[test]
fn self_dirty_jobs_cannot_be_reentered_by_nested_admission() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let nested = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 0))
            .unwrap()
            .state,
        Availability::Pending
    );
    assert_eq!(
        run.publish(&mut channel, Scoped::new(scope, 0), 9).unwrap(),
        Change::Changed
    );
    {
        let (kernel, meter) = run.parts();
        assert_eq!(kernel.next(meter).unwrap(), Some(nested));
        kernel
            .begin(nested, meter)
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
        assert_eq!(kernel.next(meter).unwrap(), None);
        assert!(
            matches!(kernel.closure(), Closure::Open(open) if open.pending == 1 && open.in_flight == 1)
        );
    }
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.pending == 1));
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(job));
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = run.query(&mut channel, Scoped::new(scope, 0)).unwrap();
    assert_eq!(before.value(), Some(&9));
    assert_eq!(
        run.publish(&mut channel, Scoped::new(scope, 0), 9).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 0))
            .unwrap()
            .version,
        before.version
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), None);
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn admitted_interrupted_unsealed_and_external_obligations_cannot_fabricate_closure() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.unsealed == 1));
    let job = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(job));
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.in_flight == 1 && open.pending == 0)
    );
    drop(kernel.begin(job, &mut meter).unwrap());
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
    kernel.retry(job, &mut meter).unwrap();
    kernel
        .begin(job, &mut meter)
        .unwrap()
        .finish(LocalClosure::Limited)
        .unwrap();
    let frontier = kernel.obligation(scope, &mut meter).unwrap();
    assert!(kernel.local_limits());
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.obligations == 1));
    kernel.resolve_obligation(frontier, &mut meter).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn generation_retirement_invalidates_facts_and_wakes_cross_scope_consumers() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let old = scope(&mut kernel, &mut meter, 0);
    let new = scope(&mut kernel, &mut meter, 1);
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let writer = kernel.register(old, (), &mut meter).unwrap();
    let consumer = kernel.register(new, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(old, 0), 2).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(consumer, &mut meter).unwrap();
    run.query(&mut channel, Scoped::new(old, 0)).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel.retire_scope(old, &mut meter).unwrap();
    assert!(kernel.worklist().contains(&consumer));
    assert!(matches!(
        kernel.begin(writer, &mut meter),
        Err(KernelError::InvalidJob)
    ));
    let mut run = kernel.begin(consumer, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(old, 0)),
        Err(KernelError::InactiveScope)
    );
    run.publish(&mut channel, Scoped::new(new, 0), 7).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(new, 0))
            .unwrap()
            .value(),
        Some(&7)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut other = Kernel::new().unwrap();
    let other_scope = scope(&mut other, &mut meter, 1);
    assert_eq!(other_scope.key(), new.key());
    assert_ne!(other_scope.epoch(), new.epoch());
    let other_job = other.register(other_scope, (), &mut meter).unwrap();
    let mut run = other.begin(other_job, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut channel, Scoped::new(new, 0)),
        Err(KernelError::WrongEpoch)
    );
    run.finish(LocalClosure::Stable).unwrap();
}

#[derive(Clone, Debug)]
struct Decorated {
    meaning: u32,
    display_revision: u32,
}
struct DecoratedFacts;
impl analysis_kernel::Capability for DecoratedFacts {
    type Key = ();
    type Value = Decorated;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Decorated) -> usize {
        2
    }
}
struct SemanticMaximum;
impl analysis_kernel::JoinPolicy for SemanticMaximum {
    type Value = Decorated;
    type Error = std::convert::Infallible;
    fn work(&self, _: &Decorated, _: &Decorated) -> usize {
        4
    }
    fn join(&self, a: &Decorated, b: &Decorated) -> Result<Decorated, Self::Error> {
        Ok(if a.meaning > b.meaning {
            a.clone()
        } else {
            b.clone()
        })
    }
    fn equivalent(&self, a: &Decorated, b: &Decorated) -> bool {
        a.meaning == b.meaning
    }
}
#[test]
fn representation_changes_do_not_advance_semantic_versions_or_requeue_readers() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let mut values = kernel
        .channel::<DecoratedFacts, _, _>(Ascending(SemanticMaximum), &mut meter)
        .unwrap();
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let reader = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(
        &mut values,
        Scoped::new(scope, ()),
        Decorated {
            meaning: 7,
            display_revision: 0,
        },
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.query(&mut values, Scoped::new(scope, ())).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    assert_eq!(
        run.publish(
            &mut values,
            Scoped::new(scope, ()),
            Decorated {
                meaning: 7,
                display_revision: 100
            }
        )
        .unwrap(),
        Change::Unchanged
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!kernel.worklist().contains(&reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let after = run.query(&mut values, Scoped::new(scope, ())).unwrap();
    assert_eq!(after.version, before.version);
    assert_eq!(after.value().unwrap().display_revision, 0);
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn external_state_output_and_caller_template_changes_share_the_dependency_core() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let output_meta = kernel.slot::<Vec<u32>, _>(scope, &mut meter).unwrap();
    let caller_a_meta = kernel.slot::<Vec<u32>, _>(scope, &mut meter).unwrap();
    let caller_b_meta = kernel.slot::<Vec<u32>, _>(scope, &mut meter).unwrap();
    let mut output = vec![2];
    let mut caller_a = vec![10];
    let caller_b = vec![20];
    let publisher = kernel.register(scope, (), &mut meter).unwrap();
    let resume_a = kernel.register(scope, (), &mut meter).unwrap();
    let resume_b = kernel.register(scope, (), &mut meter).unwrap();
    for (job, template, value) in [
        (resume_a, &caller_a_meta, &caller_a),
        (resume_b, &caller_b_meta, &caller_b),
    ] {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.read_slot(&output_meta, &output, output.len()).unwrap();
        run.read_slot(template, value, value.len()).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    let mut run = kernel.begin(publisher, &mut meter).unwrap();
    run.publish_prepared(
        &caller_a_meta,
        &mut caller_a,
        vec![10, 11],
        Change::Changed,
        2,
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&resume_a));
    assert!(!kernel.worklist().contains(&resume_b));
    let mut run = kernel.begin(resume_a, &mut meter).unwrap();
    let old_version = run
        .read_slot(&output_meta, &output, output.len())
        .unwrap()
        .version;
    assert_eq!(
        run.read_slot(&caller_a_meta, &caller_a, caller_a.len())
            .unwrap()
            .value(),
        Some(&vec![10, 11])
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(publisher, &mut meter).unwrap();
    run.meter().fail_on = Some(run.meter().calls + 1);
    assert_eq!(
        run.publish_prepared(&output_meta, &mut output, vec![2, 3], Change::Changed, 2),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(run.slot_version(&output_meta).unwrap(), old_version);
    drop(run);
    assert_eq!(output, [2]);
    assert!(!kernel.worklist().contains(&resume_a));
    assert!(!kernel.worklist().contains(&resume_b));
    meter.fail_on = None;
    kernel.retry(publisher, &mut meter).unwrap();
    let mut run = kernel.begin(publisher, &mut meter).unwrap();
    run.publish_prepared(&output_meta, &mut output, vec![2, 3], Change::Changed, 2)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&resume_a));
    assert!(kernel.worklist().contains(&resume_b));
    assert_eq!(
        caller_b,
        [20],
        "independent caller-owned templates are not transplanted"
    );
}

#[test]
fn policy_cost_overflow_never_installs_a_partial_value() {
    struct TooMuch;
    impl analysis_kernel::JoinPolicy for TooMuch {
        type Value = u32;
        type Error = std::convert::Infallible;
        fn work(&self, _: &u32, _: &u32) -> usize {
            usize::MAX
        }
        fn join(&self, _: &u32, _: &u32) -> Result<u32, Self::Error> {
            panic!("unfunded computation must not execute")
        }
        fn equivalent(&self, _: &u32, _: &u32) -> bool {
            false
        }
    }
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 0);
    let mut channel = kernel
        .channel::<Numbers, _, _>(Ascending(TooMuch), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(scope, 0), 1).unwrap();
    let before = run.query(&mut channel, Scoped::new(scope, 0)).unwrap();
    assert_eq!(
        run.publish(&mut channel, Scoped::new(scope, 0), 2),
        Err(PublishError::Kernel(KernelError::CostOverflow))
    );
    assert_eq!(
        run.query(&mut channel, Scoped::new(scope, 0)).unwrap(),
        before
    );
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn product_and_lift_keep_bottom_separate_from_uncomputed_answers() {
    use analysis_kernel::{JoinPolicy, Lift, Product, Reachable};
    let policy = Lift(Product(Maximum, Maximum));
    let a = Reachable::Value((2, 9));
    let b = Reachable::Value((7, 4));
    assert_eq!(policy.join(&Reachable::Bottom, &a).unwrap(), a);
    assert_eq!(policy.join(&a, &b).unwrap(), Reachable::Value((7, 9)));
    assert!(policy.equivalent(&policy.join(&a, &a).unwrap(), &a));
    assert_eq!(policy.join(&a, &b).unwrap(), policy.join(&b, &a).unwrap());
    let c = Reachable::Value((5, 11));
    assert_eq!(
        policy.join(&policy.join(&a, &b).unwrap(), &c).unwrap(),
        policy.join(&a, &policy.join(&b, &c).unwrap()).unwrap()
    );
    assert_ne!(
        Availability::Value(Reachable::<u8>::Bottom),
        Availability::Pending
    );
}

#[path = "support/meter.rs"]
mod meter;
