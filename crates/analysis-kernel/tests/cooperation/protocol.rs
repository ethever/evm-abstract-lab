use super::support::{Conflict, Ledger, Maximum, Numbers, RuleError, Stop, scope};
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Availability, Capability, Change, Closure, Cons, Context, Here,
    Kernel, KernelError, Lift, LocalClosure, Nil, PublishError, Reachable, Read, Scope, Scoped,
    There, solve,
};

#[test]
fn self_dirty_run_survives_finish_and_equal_publication_does_not_spin() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 1, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let job = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0)).unwrap().state,
        Availability::Pending
    );
    assert_eq!(
        run.publish(&mut values, Scoped::new(scope, 0), 7).unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.pending == 1));
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(job));
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    assert_eq!(before.value(), Some(&7));
    assert_eq!(
        run.publish(&mut values, Scoped::new(scope, 0), 7).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0))
            .unwrap()
            .version,
        before.version
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), None);
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn nested_a_to_b_to_running_a_notification_is_not_lost() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 2, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let a = kernel.register(scope, 0u8, &mut meter).unwrap();
    let b = kernel.register(scope, 1u8, &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run_a = kernel.begin(a, &mut meter).unwrap();
    run_a.query(&mut values, Scoped::new(scope, 1)).unwrap();
    run_a
        .publish(&mut values, Scoped::new(scope, 0), 1)
        .unwrap();
    {
        let (kernel, meter) = run_a.parts();
        let mut run_b = kernel.begin(b, meter).unwrap();
        assert_eq!(
            run_b
                .query(&mut values, Scoped::new(scope, 0))
                .unwrap()
                .value(),
            Some(&1)
        );
        run_b
            .publish(&mut values, Scoped::new(scope, 1), 2)
            .unwrap();
        run_b.finish(LocalClosure::Stable).unwrap();
    }
    run_a.finish(LocalClosure::Stable).unwrap();
    let mut counts = [1usize, 1];
    while let Some(job) = kernel.next(&mut meter).unwrap() {
        let role = *kernel.task::<Stop>(job).unwrap();
        let mut run = kernel.begin(job, &mut meter).unwrap();
        let value = run
            .query(&mut values, Scoped::new(scope, 1 - role))
            .unwrap()
            .value()
            .copied()
            .unwrap();
        run.publish(&mut values, Scoped::new(scope, role), (value + 1).min(3))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        counts[role as usize] += 1;
        assert!(
            counts.iter().sum::<usize>() < 10,
            "unchanged answers must quiesce"
        );
    }
    assert_eq!(counts, [3, 2]);
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn a_missing_read_retains_an_obligation_and_late_producer_wakes_the_reader() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 3, 0);
    let mut values = kernel
        .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let reader = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 8)).unwrap().state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.missing == 1 && open.pending == 0)
    );
    let provider = kernel.register(scope, (), &mut meter).unwrap();
    kernel
        .provide(&mut values, Scoped::new(scope, 8), provider, &mut meter)
        .unwrap();
    let mut run = kernel.begin(provider, &mut meter).unwrap();
    run.publish(&mut values, Scoped::new(scope, 8), 42).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 8))
            .unwrap()
            .value(),
        Some(&42)
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

struct Lifted;
impl Capability for Lifted {
    type Key = u8;
    type Value = Reachable<u32>;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &Self::Value) -> usize {
        2
    }
}

#[test]
fn pending_absent_bottom_and_top_are_distinct_and_a_provider_cannot_erase_another() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 4, 0);
    // In this deliberately finite example 15 denotes the full four-bit universe.
    let mut values = kernel
        .channel::<Lifted, _, _>(Ascending(Lift(Maximum)), &mut meter)
        .unwrap();
    let reader = kernel.register(scope, (), &mut meter).unwrap();
    let first = kernel.register(scope, (), &mut meter).unwrap();
    let second = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let pending = run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    assert_eq!(pending.state, Availability::Pending);
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(first, &mut meter).unwrap();
    run.resolve_absent(&mut values, Scoped::new(scope, 0))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let absent = run.query(&mut values, Scoped::new(scope, 0)).unwrap();
    assert_eq!(absent.state, Availability::Absent);
    assert_ne!(absent.version, pending.version);
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(second, &mut meter).unwrap();
    run.publish(&mut values, Scoped::new(scope, 0), Reachable::Bottom)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0)).unwrap().state,
        Availability::Value(Reachable::Bottom)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(second, &mut meter).unwrap();
    run.publish(&mut values, Scoped::new(scope, 0), Reachable::Value(15))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(first, &mut meter).unwrap();
    assert_eq!(
        run.resolve_absent(&mut values, Scoped::new(scope, 0)),
        Err(KernelError::CannotErase)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut values, Scoped::new(scope, 0)).unwrap().state,
        Availability::Value(Reachable::Value(15))
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

struct Total;
impl Capability for Total {
    type Key = u8;
    type Value = u32;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &u32) -> usize {
        1
    }
}
struct Aggregate {
    evaluations: usize,
}
impl<Cx> Analyzer<Cx> for Aggregate
where
    Cx: Read<Numbers, Here, Error = KernelError<Stop>>
        + Read<Numbers, There<Here>, Error = KernelError<Stop>>
        + Accepts<Total, There<There<Here>>, Error = PublishError<Conflict, Stop>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn evaluate(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.evaluations += 1;
        let a = <Cx as Read<Numbers>>::query(cx, Scoped::new(*scope, 0))?;
        let b = <Cx as Read<Numbers, There<Here>>>::query(cx, Scoped::new(*scope, 0))?;
        if let (Some(a), Some(b)) = (a.value(), b.value()) {
            <Cx as Accepts<Total, There<There<Here>>>>::accept(cx, Scoped::new(*scope, 0), a + b)?;
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn multiple_providers_have_explicit_routes_and_late_updates_recompute_the_aggregate() {
    for initial in [[2, 3], [3, 2]] {
        let mut meter = Ledger::default();
        let mut kernel = Kernel::new().unwrap();
        let scope = scope(&mut kernel, &mut meter, 5, 0);
        let a = kernel
            .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        let b = kernel
            .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        let total = kernel
            .channel::<Total, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        let mut stores = Cons::new(a, Cons::new(b, Cons::new(total, Nil)));
        let aggregate_job = kernel.register(scope, scope, &mut meter).unwrap();
        let mut aggregate = Aggregate { evaluations: 0 };
        assert!(matches!(
            solve(&mut kernel, &mut aggregate, &mut stores, &mut meter, 10)
                .unwrap()
                .closure,
            Closure::Open(_)
        ));
        let writer = kernel.register(scope, scope, &mut meter).unwrap();
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        run.publish(&mut stores.head, Scoped::new(scope, 0), initial[0])
            .unwrap();
        run.publish(&mut stores.tail.head, Scoped::new(scope, 0), initial[1])
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        kernel.seal_scope(scope, &mut meter).unwrap();
        assert_eq!(
            solve(&mut kernel, &mut aggregate, &mut stores, &mut meter, 10)
                .unwrap()
                .closure,
            Closure::Closed
        );
        assert!(aggregate.evaluations >= 2);
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        assert_eq!(
            run.query(&mut stores.tail.tail.head, Scoped::new(scope, 0))
                .unwrap()
                .value(),
            Some(&5)
        );
        run.publish(&mut stores.head, Scoped::new(scope, 0), 7)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(kernel.next(&mut meter).unwrap(), Some(aggregate_job));
        // solve may include a harmless rerun of the observer, because its own read
        // is tracked too. Every task uses the same independently declared rules.
        let mut run = kernel.begin(aggregate_job, &mut meter).unwrap();
        aggregate
            .evaluate(&scope, &mut Context::new(&mut run, &mut stores))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        assert_eq!(
            run.query(&mut stores.tail.tail.head, Scoped::new(scope, 0))
                .unwrap()
                .value(),
            Some(&(7 + initial[1]))
        );
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(kernel.closure(), Closure::Closed);
    }
}

#[test]
fn dynamic_successor_after_initial_seal_is_real_pending_work_and_explicit_frontier_survives_empty_queue()
 {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 6, 0);
    let parent = kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut run = kernel.begin(parent, &mut meter).unwrap();
    let (kernel_ref, meter_ref) = run.parts();
    let child = kernel_ref.register(scope, (), meter_ref).unwrap();
    let frontier = kernel_ref.obligation(scope, meter_ref).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(child));
    kernel
        .begin(child, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert!(
        matches!(kernel.closure(), Closure::Open(open) if open.pending == 0 && open.obligations == 1)
    );
    kernel.resolve_obligation(frontier, &mut meter).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}
