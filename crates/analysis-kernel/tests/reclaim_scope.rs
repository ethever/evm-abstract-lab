//! Ownership contracts for composed stores across scope retirement and retry.

use analysis_kernel::{
    Ascending, Capability, Channel, Cons, JobId, JoinPolicy, Kernel, KernelError, LocalClosure,
    Meter, Nil, ReclaimScope, Scope, ScopeKey, Scoped,
};
use std::{
    convert::Infallible,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Work,
    Cancelled,
}

#[derive(Default)]
struct Ledger {
    used: usize,
    reservations: usize,
    refuse_above: Option<usize>,
    cancelled: bool,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.reservations += 1;
        if self.refuse_above.is_some_and(|limit| work > limit) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(work).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
struct Payload {
    bytes: Vec<u8>,
    drops: Arc<AtomicUsize>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
fn payload(drops: &Arc<AtomicUsize>, size: usize, marker: u8) -> Arc<Payload> {
    Arc::new(Payload {
        bytes: vec![marker; size],
        drops: Arc::clone(drops),
    })
}

struct Routes;
impl Capability for Routes {
    type Key = u8;
    type Value = Arc<Payload>;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &Arc<Payload>) -> usize {
        8
    }
    fn drop_work(value: &Arc<Payload>) -> usize {
        value.bytes.len().saturating_add(8)
    }
}
#[derive(Clone, Debug)]
struct Summary(Arc<Payload>);

struct Summaries;
impl Capability for Summaries {
    type Key = &'static str;
    type Value = Summary;
    fn key_work(key: &&'static str) -> usize {
        key.len().saturating_add(1)
    }
    fn value_work(_: &Summary) -> usize {
        8
    }
    fn drop_work(value: &Summary) -> usize {
        Routes::drop_work(&value.0)
    }
}

struct Maximum;
impl JoinPolicy for Maximum {
    type Value = Arc<Payload>;
    type Error = Infallible;
    fn work(&self, a: &Arc<Payload>, b: &Arc<Payload>) -> usize {
        a.bytes
            .len()
            .saturating_add(b.bytes.len())
            .saturating_add(16)
    }
    fn join(&self, a: &Arc<Payload>, b: &Arc<Payload>) -> Result<Arc<Payload>, Infallible> {
        Ok(Arc::clone(if a.bytes >= b.bytes { a } else { b }))
    }
    fn equivalent(&self, a: &Arc<Payload>, b: &Arc<Payload>) -> bool {
        a.bytes == b.bytes
    }
}

struct SummaryMaximum;
impl JoinPolicy for SummaryMaximum {
    type Value = Summary;
    type Error = Infallible;
    fn work(&self, a: &Summary, b: &Summary) -> usize {
        Maximum.work(&a.0, &b.0)
    }
    fn join(&self, a: &Summary, b: &Summary) -> Result<Summary, Infallible> {
        Maximum.join(&a.0, &b.0).map(Summary)
    }
    fn equivalent(&self, a: &Summary, b: &Summary) -> bool {
        Maximum.equivalent(&a.0, &b.0)
    }
}

type Stores = Cons<
    Channel<Routes, Ascending<Maximum>>,
    Cons<Channel<Summaries, Ascending<SummaryMaximum>>, Nil>,
>;

fn scope<J>(kernel: &mut Kernel<J>, meter: &mut Ledger, context: u64) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context,
                generation: 0,
                policy: 1,
            },
            meter,
        )
        .unwrap()
}

struct Fixture {
    kernel: Kernel<&'static str>,
    stores: Stores,
    meter: Ledger,
    local: Scope,
    world: Scope,
    reader: JobId,
    route_drops: Arc<AtomicUsize>,
    summary_drops: Arc<AtomicUsize>,
    world_drops: Arc<AtomicUsize>,
    world_route: Weak<Payload>,
    world_summary: Weak<Payload>,
}
impl Fixture {
    fn new() -> Self {
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Ledger::default();
        let local = scope(&mut kernel, &mut meter, 1);
        let world = scope(&mut kernel, &mut meter, 0);
        let writer = kernel.register(local, "local", &mut meter).unwrap();
        let reader = kernel.register(world, "reader", &mut meter).unwrap();
        kernel.seal_scope(local, &mut meter).unwrap();
        kernel.seal_scope(world, &mut meter).unwrap();
        let mut routes = kernel
            .channel::<Routes, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        let mut summaries = kernel
            .channel::<Summaries, _, _>(Ascending(SummaryMaximum), &mut meter)
            .unwrap();
        let route_drops = Arc::new(AtomicUsize::new(0));
        let summary_drops = Arc::new(AtomicUsize::new(0));
        let world_drops = Arc::new(AtomicUsize::new(0));
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        for key in [1, 2] {
            run.publish(
                &mut routes,
                Scoped::new(local, key),
                payload(&route_drops, 1024, key),
            )
            .unwrap();
        }
        run.publish(
            &mut summaries,
            Scoped::new(local, "entry"),
            Summary(payload(&summary_drops, 16384, 3)),
        )
        .unwrap();
        // The retiring local publisher also contributes world-scoped facts.
        let marker = payload(&world_drops, 64, 42);
        let world_route = Arc::downgrade(&marker);
        run.publish(&mut routes, Scoped::new(world, 1), marker)
            .unwrap();
        let marker = payload(&world_drops, 64, 73);
        let world_summary = Arc::downgrade(&marker);
        run.publish(&mut summaries, Scoped::new(world, "entry"), Summary(marker))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        Self {
            kernel,
            stores: Cons::new(routes, Cons::new(summaries, Nil)),
            meter,
            local,
            world,
            reader,
            route_drops,
            summary_drops,
            world_drops,
            world_route,
            world_summary,
        }
    }

    fn retained(&self) -> (usize, usize) {
        (self.stores.head.len(), self.stores.tail.head.len())
    }

    fn dropped(&self) -> (usize, usize, usize) {
        (
            self.route_drops.load(Ordering::SeqCst),
            self.summary_drops.load(Ordering::SeqCst),
            self.world_drops.load(Ordering::SeqCst),
        )
    }
}

#[test]
fn heterogeneous_stores_release_retired_payloads_and_preserve_world_markers() {
    let mut f = Fixture::new();
    assert_eq!(f.retained(), (3, 2));
    assert_eq!(f.dropped(), (0, 0, 0));
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    let before_route = run
        .query(&mut f.stores.head, Scoped::new(f.world, 1))
        .unwrap();
    let before_summary = run
        .query(&mut f.stores.tail.head, Scoped::new(f.world, "entry"))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    f.kernel.retire_scope(f.local, &mut f.meter).unwrap();
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Ok(3)
    );
    assert_eq!(f.retained(), (1, 1));
    assert_eq!(f.dropped(), (2, 1, 0));
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Ok(0)
    );
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    let route = run
        .query(&mut f.stores.head, Scoped::new(f.world, 1))
        .unwrap();
    let summary = run
        .query(&mut f.stores.tail.head, Scoped::new(f.world, "entry"))
        .unwrap();
    assert!(Arc::ptr_eq(
        route.value().unwrap(),
        before_route.value().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &summary.value().unwrap().0,
        &before_summary.value().unwrap().0
    ));
    assert_eq!(route.version, before_route.version);
    assert_eq!(summary.version, before_summary.version);
    assert_eq!(route.value().unwrap().bytes[0], 42);
    assert_eq!(summary.value().unwrap().0.bytes[0], 73);
    assert_eq!(
        run.query(&mut f.stores.head, Scoped::new(f.local, 1))
            .unwrap_err(),
        KernelError::InactiveScope
    );
    run.finish(LocalClosure::Stable).unwrap();
    drop((before_route, before_summary, route, summary));
    assert_eq!(f.world_route.strong_count(), 1);
    assert_eq!(f.world_summary.strong_count(), 1);
    drop(f.stores);
    assert_eq!(f.world_drops.load(Ordering::SeqCst), 2);
    assert!(f.world_route.upgrade().is_none());
    assert!(f.world_summary.upgrade().is_none());
}

#[test]
fn composed_reclaim_inherits_active_and_foreign_scope_refusals() {
    let mut f = Fixture::new();
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Err(KernelError::ScopeNotRetired)
    );
    let mut other = Kernel::<()>::new().unwrap();
    let foreign = scope(&mut other, &mut f.meter, 1);
    other.retire_scope(foreign, &mut f.meter).unwrap();
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, foreign, &mut f.meter),
        Err(KernelError::WrongEpoch)
    );
    assert_eq!(
        f.stores.reclaim_scope(&mut other, foreign, &mut f.meter),
        Err(KernelError::WrongEpoch)
    );
    let mut fork = f.kernel.fork(&mut f.meter).unwrap();
    let abandoned = scope(fork.kernel(), &mut f.meter, 99);
    fork.kernel().retire_scope(abandoned, &mut f.meter).unwrap();
    drop(fork);
    let actual = scope(&mut f.kernel, &mut f.meter, 99);
    f.kernel.retire_scope(actual, &mut f.meter).unwrap();
    assert_ne!(abandoned, actual);
    assert_eq!(
        f.stores
            .reclaim_scope(&mut f.kernel, abandoned, &mut f.meter),
        Err(KernelError::InactiveScope)
    );
    assert_eq!(f.retained(), (3, 2));
    assert_eq!(f.dropped(), (0, 0, 0));
    f.kernel.retire_scope(f.local, &mut f.meter).unwrap();
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Ok(3)
    );
    assert_eq!(f.retained(), (1, 1));
    assert_eq!(f.dropped(), (2, 1, 0));
}

#[test]
fn second_channel_drop_refusal_keeps_partial_progress_and_retry_is_idempotent() {
    let mut f = Fixture::new();
    f.kernel.retire_scope(f.local, &mut f.meter).unwrap();
    let before = f.meter.used;
    // The two 1 KiB route drops fit; the summary's last 16 KiB Arc does not.
    // Query/clone costs are only eight units, so this also requires drop_work.
    f.meter.refuse_above = Some(8192);
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert!(f.meter.used > before);
    assert_eq!(f.retained(), (1, 2));
    assert_eq!(f.dropped(), (2, 0, 0));
    assert_eq!(f.world_route.strong_count(), 1);
    assert_eq!(f.world_summary.strong_count(), 1);
    let paid = f.meter.used;
    f.meter.refuse_above = None;
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Ok(1)
    );
    assert!(f.meter.used > paid);
    assert_eq!(f.retained(), (1, 1));
    assert_eq!(f.dropped(), (2, 1, 0));
    assert_eq!(
        f.stores.reclaim_scope(&mut f.kernel, f.local, &mut f.meter),
        Ok(0)
    );
    assert_eq!(f.retained(), (1, 1));
    assert_eq!(f.dropped(), (2, 1, 0));
    drop(f.stores);
    assert_eq!(f.world_drops.load(Ordering::SeqCst), 2);
}

#[test]
fn nil_checks_cancellation_without_reserving_work_or_validating_a_scope() {
    let mut kernel = Kernel::<()>::new().unwrap();
    let mut setup = Ledger::default();
    let active = scope(&mut kernel, &mut setup, 1);
    let mut meter = Ledger {
        refuse_above: Some(0),
        ..Ledger::default()
    };
    assert_eq!(Nil.reclaim_scope(&mut kernel, active, &mut meter), Ok(0));
    assert_eq!(meter.reservations, 0);
    assert_eq!(meter.used, 0);
    meter.cancelled = true;
    assert_eq!(
        Nil.reclaim_scope(&mut kernel, active, &mut meter),
        Err(KernelError::Stopped(Stop::Cancelled))
    );
    assert_eq!(meter.reservations, 0);
    assert_eq!(meter.used, 0);
}

#[test]
fn composed_counts_refuse_overflow_after_children_succeed_and_retry_returns_zero() {
    // Public implementations can return counts too large to model with real
    // allocations. Taking each count keeps this boundary test idempotent.
    struct Count(usize);
    impl ReclaimScope for Count {
        fn reclaim_scope<J, M: Meter>(
            &mut self,
            _: &mut Kernel<J>,
            _: Scope,
            meter: &mut M,
        ) -> Result<usize, KernelError<M::Stop>> {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            Ok(std::mem::take(&mut self.0))
        }
    }
    let mut kernel = Kernel::<()>::new().unwrap();
    let mut meter = Ledger::default();
    let retired = scope(&mut kernel, &mut meter, 1);
    kernel.retire_scope(retired, &mut meter).unwrap();
    let mut stores = Cons::new(Count(usize::MAX), Cons::new(Count(1), Nil));
    assert_eq!(
        stores.reclaim_scope(&mut kernel, retired, &mut meter),
        Err(KernelError::CostOverflow)
    );
    assert_eq!((stores.head.0, stores.tail.head.0), (0, 0));
    assert_eq!(
        stores.reclaim_scope(&mut kernel, retired, &mut meter),
        Ok(0)
    );
}
