use super::support::{Budget, Stop, scope};
use analysis_kernel::{
    Ascending, Capability, Change, JoinPolicy, Kernel, KernelError, LocalClosure, Meter,
    PublishError, Scoped,
};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug)]
struct Payload {
    size: usize,
    drops: Arc<AtomicUsize>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct Shared;
impl Capability for Shared {
    type Key = ();
    type Value = Arc<Payload>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Arc<Payload>) -> usize {
        8
    }
    fn drop_work(value: &Arc<Payload>) -> usize {
        value.size
    }
}
struct Maximum;
impl JoinPolicy for Maximum {
    type Value = Arc<Payload>;
    type Error = Infallible;
    fn work(&self, _: &Arc<Payload>, _: &Arc<Payload>) -> usize {
        16
    }
    fn join(&self, a: &Arc<Payload>, b: &Arc<Payload>) -> Result<Arc<Payload>, Infallible> {
        Ok(Arc::clone(if a.size > b.size { a } else { b }))
    }
    fn equivalent(&self, a: &Arc<Payload>, b: &Arc<Payload>) -> bool {
        a.size == b.size
    }
}
struct Limit(usize);
impl Meter for Limit {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        self.0 = self.0.checked_sub(cost).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        Ok(())
    }
}

#[test]
fn typed_replacement_pays_final_old_payload_drop_with_exact_atomic_denial() {
    let mut quote = None;
    for allowance in [usize::MAX, 0, 1] {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let scope = scope(&mut kernel, &mut meter, 0);
        let job = kernel.register(scope, (), &mut meter).unwrap();
        let mut channel = kernel
            .channel::<Shared, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        let old = Arc::new(Payload {
            size: 4096,
            drops: Arc::clone(&drops),
        });
        let old_weak = Arc::downgrade(&old);
        let incoming = Arc::new(Payload {
            size: 8192,
            drops: Arc::clone(&drops),
        });
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.publish(&mut channel, Scoped::new(scope, ()), old)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let remaining = if allowance == usize::MAX {
            usize::MAX
        } else {
            quote.unwrap() - 1 + allowance
        };
        let mut limit = Limit(remaining);
        let mut run = kernel.begin(job, &mut limit).unwrap();
        let result = run.publish(&mut channel, Scoped::new(scope, ()), Arc::clone(&incoming));
        let used = remaining - run.meter().0;
        if allowance == 0 {
            assert_eq!(
                result,
                Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
            );
            assert!(old_weak.upgrade().is_some());
            assert_eq!(drops.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(result.unwrap(), Change::Changed);
            assert!(old_weak.upgrade().is_none());
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            if let Some(quote) = quote {
                assert_eq!(quote, used);
            } else {
                quote = Some(used);
            }
            assert!(used >= 4096);
        }
    }
}
