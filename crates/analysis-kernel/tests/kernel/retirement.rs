use super::support::{Budget, Stop, scope};
use analysis_kernel::{Closure, Kernel, KernelError, LocalClosure};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Payload {
    drops: Arc<AtomicUsize>,
    value: usize,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
fn payload(drops: &Arc<AtomicUsize>) -> Payload {
    Payload {
        drops: Arc::clone(drops),
        value: 42,
    }
}

#[test]
fn retirement_releases_tasks_but_keeps_checked_handles_and_tokens() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let old = scope(&mut kernel, &mut meter, 0);
    let idle = kernel.register(old, payload(&drops), &mut meter).unwrap();
    let suspended = kernel.register(old, payload(&drops), &mut meter).unwrap();
    let detached = kernel.task_handle(idle, &mut meter).unwrap();
    let mut token = kernel.begin(suspended, &mut meter).unwrap().suspend();
    kernel.retire_scope(old, &mut meter).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(detached.value, 42, "an external Arc still owns its payload");
    assert_eq!(kernel.statistics().jobs, 2, "identity tombstones remain");
    assert_eq!(kernel.worklist().records(), 0);
    assert_eq!(kernel.closure(), Closure::Closed);
    for job in [idle, suspended] {
        assert!(matches!(
            kernel.task::<Stop>(job),
            Err(KernelError::InvalidJob)
        ));
        assert!(matches!(
            kernel.task_handle(job, &mut meter),
            Err(KernelError::InvalidJob)
        ));
        assert!(matches!(
            kernel.begin(job, &mut meter),
            Err(KernelError::InvalidJob)
        ));
        assert_eq!(kernel.retry(job, &mut meter), Err(KernelError::InvalidJob));
    }
    assert!(matches!(
        kernel.resume(&mut token, &mut meter),
        Err(KernelError::InvalidJob)
    ));
    let next = scope(&mut kernel, &mut meter, 1);
    let live = kernel.register(next, payload(&drops), &mut meter).unwrap();
    assert_ne!(live, idle);
    assert_eq!(kernel.task::<Stop>(live).unwrap().value, 42);
    drop(detached);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(kernel);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
fn retirement_refusal_keeps_payloads_and_queue_untouched() {
    for fail_at in 1..=2 {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let old = scope(&mut kernel, &mut meter, 0);
        let job = kernel.register(old, payload(&drops), &mut meter).unwrap();
        let before = kernel.worklist().clone();
        meter.fail_on = Some(meter.calls + fail_at);
        assert_eq!(
            kernel.retire_scope(old, &mut meter),
            Err(KernelError::Stopped(Stop::Work))
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(kernel.task::<Stop>(job).unwrap().value, 42);
        assert_eq!(kernel.worklist(), &before);
        meter.fail_on = None;
        kernel.retire_scope(old, &mut meter).unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn fork_retirement_rollback_and_commit_preserve_task_ownership() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let old = scope(&mut kernel, &mut meter, 0);
    let job = kernel.register(old, payload(&drops), &mut meter).unwrap();
    let mut token = kernel.begin(job, &mut meter).unwrap().suspend();
    let mut fork = kernel.fork(&mut meter).unwrap();
    fork.kernel().retire_scope(old, &mut meter).unwrap();
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "live graph still owns task"
    );
    assert_eq!(kernel.task::<Stop>(job).unwrap().value, 42);
    drop(fork);
    kernel
        .resume(&mut token, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    assert_eq!(fork.view().task::<Stop>(job).unwrap().value, 42);
    kernel.retire_scope(old, &mut meter).unwrap();
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "fork still owns its snapshot"
    );
    fork.kernel()
        .begin(job, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    drop(fork);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let next = scope(&mut kernel, &mut meter, 1);
    let next_job = kernel.register(next, payload(&drops), &mut meter).unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    fork.kernel().retire_scope(next, &mut meter).unwrap();
    kernel.commit(fork, &mut meter).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert!(matches!(
        kernel.task::<Stop>(next_job),
        Err(KernelError::InvalidJob)
    ));
    let fork = kernel.fork(&mut meter).unwrap(); // retired payloads are absent in snapshots
    assert_eq!(fork.view().statistics(), kernel.statistics());
}

#[test]
fn replacement_releases_task_payloads_and_targeted_cleanup_ignores_unrelated_fact_history() {
    use super::support::{Maximum, Numbers};
    use analysis_kernel::{Ascending, Scoped};
    fn retirement_cost(history: u64) -> usize {
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let live = scope(&mut kernel, &mut meter, 0);
        let mut channel = kernel
            .channel::<Numbers, _, _>(Ascending(Maximum), &mut meter)
            .unwrap();
        for generation in 1..=history {
            let old = scope(&mut kernel, &mut meter, generation);
            let job = kernel.register(old, (), &mut meter).unwrap();
            let mut run = kernel.begin(job, &mut meter).unwrap();
            run.publish(&mut channel, Scoped::new(old, 0), 1).unwrap();
            run.finish(LocalClosure::Stable).unwrap();
            kernel.retire_scope(old, &mut meter).unwrap();
            kernel
                .reclaim_channel_scope(&mut channel, old, &mut meter)
                .unwrap();
        }
        let target = scope(&mut kernel, &mut meter, history + 1);
        let job = kernel.register(target, (), &mut meter).unwrap();
        kernel
            .provide(&mut channel, Scoped::new(live, 0), job, &mut meter)
            .unwrap();
        let mut run = kernel.begin(job, &mut meter).unwrap();
        run.query(&mut channel, Scoped::new(live, 0)).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let start = meter.used;
        kernel.retire_scope(target, &mut meter).unwrap();
        meter.used - start
    }
    assert!(
        retirement_cost(10_000) < retirement_cost(0) + 1_024,
        "wake and mandatory-demand index lookups grow logarithmically, far below the 10000-record history"
    );

    let drops = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new().unwrap();
    let mut meter = Budget::default();
    let owner = scope(&mut kernel, &mut meter, 0);
    let old = scope(&mut kernel, &mut meter, 1);
    let next = scope(&mut kernel, &mut meter, 2);
    let owner_job = kernel.register(owner, payload(&drops), &mut meter).unwrap();
    let old_job = kernel.register(old, payload(&drops), &mut meter).unwrap();
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    kernel.seal_scope(next, &mut meter).unwrap();
    let mut run = kernel.begin(owner_job, &mut meter).unwrap();
    run.replace_generation(&mut entry, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(matches!(
        kernel.task::<Stop>(old_job),
        Err(KernelError::InvalidJob)
    ));
}
