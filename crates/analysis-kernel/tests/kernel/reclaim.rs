use super::support::{Budget, Stop, scope};
use analysis_kernel::{
    Ascending, Capability, Closure, JoinPolicy, Kernel, KernelError, LocalClosure, Meter, Scoped,
};
use std::{
    cell::Cell,
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug)]
struct Deep {
    bytes: Vec<u8>,
    drops: Arc<AtomicUsize>,
}
impl Drop for Deep {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct Shared;
impl Capability for Shared {
    type Key = u8;
    type Value = Arc<Deep>;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &Arc<Deep>) -> usize {
        8
    }
    fn drop_work(value: &Arc<Deep>) -> usize {
        value.bytes.len() + 8
    }
}
struct Keep;
impl JoinPolicy for Keep {
    type Value = Arc<Deep>;
    type Error = Infallible;
    fn work(&self, _: &Arc<Deep>, _: &Arc<Deep>) -> usize {
        16
    }
    fn join(&self, a: &Arc<Deep>, _: &Arc<Deep>) -> Result<Arc<Deep>, Infallible> {
        Ok(Arc::clone(a))
    }
    fn equivalent(&self, _: &Arc<Deep>, _: &Arc<Deep>) -> bool {
        true
    }
}
fn value(drops: &Arc<AtomicUsize>) -> Arc<Deep> {
    Arc::new(Deep {
        bytes: vec![42; 4096],
        drops: Arc::clone(drops),
    })
}

struct Limit {
    remaining: usize,
    cancelled: Cell<bool>,
    cancel_on_paid_drop: bool,
}
impl Meter for Limit {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        if cost > self.remaining {
            return Err(Stop::Work);
        }
        self.remaining -= cost;
        if self.cancel_on_paid_drop && cost > 4096 {
            self.cancelled.set(true);
        }
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled.get() {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn retired_scope_reclamation_prepays_last_arc_drop_and_preserves_other_values_and_dependencies() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let old = scope(&mut kernel, &mut meter, 0);
    let live = scope(&mut kernel, &mut meter, 1);
    let old_writer = kernel.register(old, (), &mut meter).unwrap();
    let writer = kernel.register(live, (), &mut meter).unwrap();
    let reader = kernel.register(live, (), &mut meter).unwrap();
    let mut channel = kernel
        .channel::<Shared, _, _>(Ascending(Keep), &mut meter)
        .unwrap();
    for (job, scope) in [(old_writer, old), (writer, live)] {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.publish(&mut channel, Scoped::new(scope, 0), value(&drops))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        kernel.seal_scope(scope, &mut meter).unwrap();
    }
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let old_handle = run.query(&mut channel, Scoped::new(old, 0)).unwrap();
    let live_answer = run.query(&mut channel, Scoped::new(live, 0)).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, live, &mut meter),
        Err(KernelError::ScopeNotRetired)
    );
    kernel.retire_scope(old, &mut meter).unwrap();
    assert!(
        kernel.worklist().contains(&reader),
        "retirement already wakes external readers"
    );
    drop(old_handle); // Only the retired channel cell owns this deep value now.
    let dependencies = kernel.dependency_count();
    let before_queue = kernel.worklist().clone();
    // A clone-only Arc quote would wrongly succeed on this remaining work.
    // The companion test also denies the full measured quotation by one unit.
    let mut denied = Limit {
        remaining: 4096,
        cancelled: Cell::new(false),
        cancel_on_paid_drop: false,
    };
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, old, &mut denied),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(channel.len(), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let mut cancelled = Limit {
        remaining: usize::MAX,
        cancelled: Cell::new(false),
        cancel_on_paid_drop: true,
    };
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, old, &mut cancelled),
        Err(KernelError::Stopped(Stop::Cancelled))
    );
    assert_eq!(channel.len(), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let before = meter.used;
    assert_eq!(
        kernel
            .reclaim_channel_scope(&mut channel, old, &mut meter)
            .unwrap(),
        1
    );
    let quote = meter.used - before;
    assert!(quote > 4096);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(channel.len(), 1);
    assert_eq!(kernel.dependency_count(), dependencies);
    assert_eq!(kernel.worklist(), &before_queue);
    assert_eq!(
        kernel
            .reclaim_channel_scope(&mut channel, old, &mut meter)
            .unwrap(),
        0
    );
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let after = run.query(&mut channel, Scoped::new(live, 0)).unwrap();
    assert!(Arc::ptr_eq(
        after.value().unwrap(),
        live_answer.value().unwrap()
    ));
    assert_eq!(after.version, live_answer.version);
    assert_eq!(
        run.query(&mut channel, Scoped::new(old, 0)).unwrap_err(),
        KernelError::InactiveScope
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
    assert_eq!(
        kernel.scope(old.key(), &mut meter),
        Err(KernelError::InactiveScope)
    );
    // Keep the measured quotation available for the exact-denial companion.
    assert!(quote < 4200);
}

#[test]
fn exact_drop_quote_refusal_is_atomic_and_one_more_unit_allows_reclamation() {
    let mut quotes = Vec::new();
    for allowance in [usize::MAX, 0, 1] {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let old = scope(&mut kernel, &mut meter, 0);
        let writer = kernel.register(old, (), &mut meter).unwrap();
        let mut channel = kernel
            .channel::<Shared, _, _>(Ascending(Keep), &mut meter)
            .unwrap();
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        run.publish(&mut channel, Scoped::new(old, 0), value(&drops))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        kernel.retire_scope(old, &mut meter).unwrap();
        let remaining = if allowance == usize::MAX {
            usize::MAX
        } else {
            quotes[0] - 1 + allowance
        };
        let mut exact = Limit {
            remaining,
            cancelled: Cell::new(false),
            cancel_on_paid_drop: false,
        };
        let result = kernel.reclaim_channel_scope(&mut channel, old, &mut exact);
        if allowance == 0 {
            assert_eq!(result, Err(KernelError::Stopped(Stop::Work)));
            assert_eq!(channel.len(), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(result.unwrap(), 1);
            quotes.push(remaining - exact.remaining);
            assert!(channel.is_empty());
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    }
    assert_eq!(quotes[0], quotes[1]);
}

#[test]
fn reclaim_rejects_wrong_epoch_and_aborted_scope_birth_without_touching_cells() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::<()>::new().unwrap();
    let mut meter = Budget::default();
    let mut channel = kernel
        .channel::<Shared, _, _>(Ascending(Keep), &mut meter)
        .unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let escaped = scope(fork.kernel(), &mut meter, 0);
    fork.kernel().retire_scope(escaped, &mut meter).unwrap();
    drop(fork);
    let current = scope(&mut kernel, &mut meter, 0);
    let writer = kernel.register(current, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(current, 0), value(&drops))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel.retire_scope(current, &mut meter).unwrap();
    assert_ne!(escaped, current);
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, escaped, &mut meter),
        Err(KernelError::InactiveScope)
    );
    let mut other = Kernel::<()>::new().unwrap();
    let other_scope = scope(&mut other, &mut meter, 0);
    other.retire_scope(other_scope, &mut meter).unwrap();
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, other_scope, &mut meter),
        Err(KernelError::WrongEpoch)
    );
    assert_eq!(
        other.reclaim_channel_scope(&mut channel, other_scope, &mut meter),
        Err(KernelError::WrongEpoch)
    );
    assert_eq!(channel.len(), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        kernel
            .reclaim_channel_scope(&mut channel, current, &mut meter)
            .unwrap(),
        1
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn overflow_in_drop_quote_preserves_retired_payload() {
    struct Overflow;
    impl Capability for Overflow {
        type Key = u8;
        type Value = Arc<Deep>;
        fn key_work(_: &u8) -> usize {
            1
        }
        fn value_work(_: &Arc<Deep>) -> usize {
            8
        }
        fn drop_work(_: &Arc<Deep>) -> usize {
            usize::MAX
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let old = scope(&mut kernel, &mut meter, 0);
    let writer = kernel.register(old, (), &mut meter).unwrap();
    let mut channel = kernel
        .channel::<Overflow, _, _>(Ascending(Keep), &mut meter)
        .unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.publish(&mut channel, Scoped::new(old, 0), value(&drops))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel.retire_scope(old, &mut meter).unwrap();
    assert_eq!(
        kernel.reclaim_channel_scope(&mut channel, old, &mut meter),
        Err(KernelError::CostOverflow)
    );
    assert_eq!(channel.len(), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
}
