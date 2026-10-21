use super::*;
use crate::{Answer, Availability, Closure, Cons, Discover, KernelError, Nil};

fn navigate<Cx: Discover<Bits>>(
    cx: &mut Cx,
    key: Scoped<u8>,
) -> Result<Answer<u8>, <Cx as Discover<Bits>>::Error> {
    cx.discover(key)
}

#[test]
fn unknown_future_discovery_is_optional_and_late_publication_wakes_same_generation() {
    let mut f = Fixture::new();
    let mut stores = Cons::new(f.inputs, Nil);
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    let answer = navigate(
        &mut Context::new(&mut run, &mut stores),
        Scoped::new(f.source, 42),
    )
    .unwrap();
    assert_eq!(answer.state, Availability::Pending);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(f.kernel.closure(), Closure::Closed);
    publish(
        &mut f.kernel,
        &mut f.meter,
        f.writer,
        &mut stores.head,
        f.source,
        42,
        1,
    );
    assert!(f.kernel.worklist().contains(&f.reader));
    assert!(!f.kernel.generation_stale(&f.entry, &mut f.meter).unwrap());
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query_discovery(&mut stores.head, Scoped::new(f.source, 42))
            .unwrap()
            .value(),
        Some(&1)
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(f.kernel.closure(), Closure::Closed);
}

#[test]
fn read_upgrades_discovery_permanently_and_required_subsets_survive_fork_and_retirement() {
    let mut f = Fixture::new();
    let key = Scoped::new(f.source, 42);
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query_discovery(&mut f.inputs, key.clone()).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(f.kernel.closure(), Closure::Closed);
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query(&mut f.inputs, key.clone()).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(f.kernel.closure(), Closure::Open(o) if o.missing == 1));
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query_discovery(&mut f.inputs, key).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let fork = f.kernel.fork(&mut f.meter).unwrap();
    assert_eq!(fork.view().closure(), f.kernel.closure());
    assert!(matches!(fork.view().closure(), Closure::Open(o) if o.missing == 1));
    drop(fork);
    let next = scope(&mut f.kernel, &mut f.meter, f.target.key().context, 1);
    f.kernel.seal_scope(next, &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    run.replace_generation(&mut f.entry, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    owner_read(&mut f.kernel, &mut f.meter, f.owner, &f.entry);
    assert_eq!(
        f.kernel.closure(),
        Closure::Closed,
        "retirement removes both wake and demand indexes"
    );
}

#[test]
fn topology_growth_requeues_discover_but_invalidates_an_adopting_reader() {
    let mut f = Fixture::new();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query(&mut f.inputs, Scoped::new(f.source, 0)).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = f.kernel.begin(f.downstream_reader, &mut f.meter).unwrap();
    run.query_discovery(&mut f.inputs, Scoped::new(f.source, 0))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
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
    assert!(!f.stale(1));
    assert!(f.kernel.worklist().contains(&f.downstream_reader));
    let mut run = f.kernel.begin(f.downstream_reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query_discovery(&mut f.inputs, Scoped::new(f.source, 0))
            .unwrap()
            .value(),
        Some(&3)
    );
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn stale_selected_route_and_early_typed_reads_are_effectively_missing_for_required_readers() {
    let mut f = Fixture::new();
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
        run.current_generation(&f.entry).unwrap().state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    let owner_scope = f.kernel.job_scope::<Stop>(f.owner).unwrap();
    assert!(
        matches!(f.kernel.closure_in::<Stop>(owner_scope).unwrap(), Closure::Open(o) if o.missing > 0)
    );
    let reader_scope = scope(&mut f.kernel, &mut f.meter, 9, 0);
    let reader = f.kernel.register(reader_scope, (), &mut f.meter).unwrap();
    f.kernel.seal_scope(reader_scope, &mut f.meter).unwrap();
    let mut run = f.kernel.begin(reader, &mut f.meter).unwrap();
    assert_eq!(
        run.query(&mut f.local, Scoped::new(f.target, 0))
            .unwrap()
            .state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        matches!(f.kernel.closure_in::<Stop>(reader_scope).unwrap(), Closure::Open(o) if o.missing > 0)
    );
}

#[test]
fn both_modes_withhold_invalid_producer_values_but_only_read_demands_their_coverage() {
    let mut f = Fixture::new();
    let anchor = f.kernel.slot::<u8, _>(f.source, &mut f.meter).unwrap();
    let mut native = 1;
    let mut shared: Inputs = f.kernel.channel(Ascending(Union), &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.observe_slot(&anchor).unwrap();
    run.publish(&mut shared, Scoped::new(f.source, 0), 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = f.kernel.begin(f.writer, &mut f.meter).unwrap();
    run.publish_prepared(&anchor, &mut native, 3, crate::Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    for required in [false, true] {
        let reader_scope = scope(&mut f.kernel, &mut f.meter, if required { 9 } else { 8 }, 0);
        let reader = f.kernel.register(reader_scope, (), &mut f.meter).unwrap();
        f.kernel.seal_scope(reader_scope, &mut f.meter).unwrap();
        let mut run = f.kernel.begin(reader, &mut f.meter).unwrap();
        let answer = if required {
            run.query(&mut shared, Scoped::new(f.source, 0))
        } else {
            run.query_discovery(&mut shared, Scoped::new(f.source, 0))
        }
        .unwrap();
        assert_eq!(answer.state, Availability::Pending);
        run.finish(LocalClosure::Stable).unwrap();
        let closure = f.kernel.closure_in::<Stop>(reader_scope).unwrap();
        if required {
            assert!(matches!(closure, Closure::Open(o) if o.missing > 0));
        } else {
            assert_eq!(closure, Closure::Closed);
        }
    }
}

#[test]
fn discovery_observes_cancel_before_interning_an_optional_future_fact() {
    let mut f = Fixture::new();
    let before = f.inputs.len();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.meter().cancelled = true;
    assert_eq!(
        run.query_discovery(&mut f.inputs, Scoped::new(f.source, 42)),
        Err(KernelError::Stopped(Stop::Cancelled))
    );
    assert_eq!(f.inputs.len(), before);
}

#[test]
fn discovery_prepays_value_clone_and_never_uses_a_refused_copy() {
    use crate::{JoinPolicy, Meter};
    use std::{cell::Cell, rc::Rc};
    struct Value {
        value: u8,
        copies: Rc<Cell<usize>>,
    }
    impl Clone for Value {
        fn clone(&self) -> Self {
            self.copies.set(self.copies.get() + 1);
            Self {
                value: self.value,
                copies: Rc::clone(&self.copies),
            }
        }
    }
    struct Costly;
    impl Capability for Costly {
        type Key = ();
        type Value = Value;
        fn key_work(_: &()) -> usize {
            1
        }
        fn value_work(_: &Value) -> usize {
            10_000
        }
    }
    struct Keep;
    impl JoinPolicy for Keep {
        type Value = Value;
        type Error = Infallible;
        fn work(&self, _: &Value, _: &Value) -> usize {
            24
        }
        fn join(&self, a: &Value, _: &Value) -> Result<Value, Infallible> {
            Ok(a.clone())
        }
        fn equivalent(&self, a: &Value, b: &Value) -> bool {
            a.value == b.value
        }
    }
    struct Ledger {
        used: usize,
        deny_clone: bool,
    }
    impl Meter for Ledger {
        type Stop = Stop;
        fn reserve(&mut self, work: usize) -> Result<(), Stop> {
            if self.deny_clone && work >= 10_000 {
                return Err(Stop::Work);
            }
            self.used += work;
            Ok(())
        }
        fn checkpoint(&self) -> Result<(), Stop> {
            Ok(())
        }
    }
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let scope = scope(&mut kernel, &mut meter, 7, 0);
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut channel = kernel
        .channel::<Costly, _, _>(Ascending(Keep), &mut meter)
        .unwrap();
    let copies = Rc::new(Cell::new(0));
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.publish(
        &mut channel,
        Scoped::new(scope, ()),
        Value {
            value: 1,
            copies: Rc::clone(&copies),
        },
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut ledger = Ledger {
        used: 0,
        deny_clone: true,
    };
    let mut run = kernel.begin(job, &mut ledger).unwrap();
    assert!(matches!(
        run.query_discovery(&mut channel, Scoped::new(scope, ())),
        Err(KernelError::Stopped(Stop::Work))
    ));
    assert_eq!(copies.get(), 0);
    drop(run);
    let spent = ledger.used;
    ledger.deny_clone = false;
    let mut run = kernel.begin(job, &mut ledger).unwrap();
    assert_eq!(
        run.query_discovery(&mut channel, Scoped::new(scope, ()))
            .unwrap()
            .value()
            .unwrap()
            .value,
        1
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(copies.get(), 1);
    assert!(ledger.used >= spent + 10_000);
    assert_eq!(kernel.closure(), Closure::Closed);
}
