use super::support::{Budget, Stop, scope};
use analysis_kernel::{
    Ascending, Capability, Cons, JoinPolicy, Kernel, KernelError, LocalClosure, Meter, Nil,
    ReclaimScope, Scoped,
};
use std::{cell::Cell, convert::Infallible, rc::Rc};

struct Payload {
    work: usize,
    drops: Rc<Cell<usize>>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
struct Shared;
impl Capability for Shared {
    type Key = u8;
    type Value = Rc<Payload>;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &Rc<Payload>) -> usize {
        1
    }
    fn drop_work(value: &Rc<Payload>) -> usize {
        value.work
    }
}
struct Maximum;
impl JoinPolicy for Maximum {
    type Value = Rc<Payload>;
    type Error = Infallible;
    fn work(&self, _: &Rc<Payload>, _: &Rc<Payload>) -> usize {
        8
    }
    fn join(&self, a: &Rc<Payload>, b: &Rc<Payload>) -> Result<Rc<Payload>, Infallible> {
        Ok(Rc::clone(if a.work > b.work { a } else { b }))
    }
    fn equivalent(&self, a: &Rc<Payload>, b: &Rc<Payload>) -> bool {
        a.work == b.work
    }
}
struct Ledger {
    used: usize,
    deny_deep_drop: bool,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        if self.deny_deep_drop && work >= 10_000 {
            return Err(Stop::Work);
        }
        self.used += work;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        Ok(())
    }
}

#[test]
fn nested_custom_stores_keep_world_values_and_retry_after_later_channel_refusal() {
    let old_drops = Rc::new(Cell::new(0));
    let world_drops = Rc::new(Cell::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let retired = scope(&mut kernel, &mut meter, 0);
    let world = scope(&mut kernel, &mut meter, 1);
    let a = kernel.register(retired, (), &mut meter).unwrap();
    let b = kernel.register(world, (), &mut meter).unwrap();
    let mut first = kernel
        .channel::<Shared, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    let mut second = kernel
        .channel::<Shared, _, _>(Ascending(Maximum), &mut meter)
        .unwrap();
    for (job, owner, drops) in [(a, retired, &old_drops), (b, world, &world_drops)] {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.publish(
            &mut first,
            Scoped::new(owner, 0),
            Rc::new(Payload {
                work: 10,
                drops: Rc::clone(drops),
            }),
        )
        .unwrap();
        run.publish(
            &mut second,
            Scoped::new(owner, 0),
            Rc::new(Payload {
                work: 10_000,
                drops: Rc::clone(drops),
            }),
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    kernel.retire_scope(retired, &mut meter).unwrap();
    let mut stores = Cons::new(Cons::new(first, Nil), Cons::new(second, Nil));
    let mut ledger = Ledger {
        used: 0,
        deny_deep_drop: true,
    };
    assert_eq!(
        stores.reclaim_scope(&mut kernel, retired, &mut ledger),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(stores.head.head.len(), 1);
    assert_eq!(stores.tail.head.len(), 2);
    assert_eq!(
        old_drops.get(),
        1,
        "earlier per-channel progress is retained honestly"
    );
    assert_eq!(world_drops.get(), 0);
    let spent = ledger.used;
    ledger.deny_deep_drop = false;
    assert_eq!(
        stores
            .reclaim_scope(&mut kernel, retired, &mut ledger)
            .unwrap(),
        1
    );
    assert!(ledger.used > spent);
    assert_eq!(
        stores
            .reclaim_scope(&mut kernel, retired, &mut ledger)
            .unwrap(),
        0
    );
    assert_eq!(old_drops.get(), 2);
    assert_eq!(world_drops.get(), 0);
    let mut run = kernel.begin(b, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut stores.head.head, Scoped::new(world, 0))
            .unwrap()
            .value()
            .unwrap()
            .work,
        10
    );
    assert_eq!(
        run.query(&mut stores.tail.head, Scoped::new(world, 0))
            .unwrap()
            .value()
            .unwrap()
            .work,
        10_000
    );
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn composite_removed_counts_reject_overflow() {
    struct Count(usize);
    impl ReclaimScope for Count {
        fn reclaim_scope<J, M: Meter>(
            &mut self,
            _: &mut Kernel<J>,
            _: analysis_kernel::Scope,
            _: &mut M,
        ) -> Result<usize, KernelError<M::Stop>> {
            Ok(self.0)
        }
    }
    let mut kernel = Kernel::<()>::new().unwrap();
    let mut meter = Budget::default();
    let retired = scope(&mut kernel, &mut meter, 0);
    kernel.retire_scope(retired, &mut meter).unwrap();
    let mut stores = Cons::new(Count(usize::MAX), Count(1));
    assert_eq!(
        stores.reclaim_scope(&mut kernel, retired, &mut meter),
        Err(KernelError::CostOverflow)
    );
}
