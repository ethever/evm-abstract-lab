use super::*;
use crate::{Availability, Closure, KernelError, Meter};
use std::cell::Cell;

fn agree(scope: Scope, f: &mut Fixture) {
    assert_eq!(
        f.kernel.closure_in_metered(scope, &mut f.meter).unwrap(),
        f.kernel.closure_in::<Stop>(scope).unwrap()
    );
}

#[test]
fn paid_local_readiness_matches_required_discovery_frontiers_and_stale_selectors() {
    let mut f = Fixture::new();
    agree(f.target, &mut f);
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query_discovery(&mut f.inputs, Scoped::new(f.source, 42))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap(),
        Closure::Closed
    );
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    run.query(&mut f.inputs, Scoped::new(f.source, 42)).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    agree(f.target, &mut f);
    let obligation = f.kernel.obligation(f.target, &mut f.meter).unwrap();
    agree(f.target, &mut f);
    assert!(
        matches!(f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap(), Closure::Open(o) if o.obligations == 1 && o.missing == 1)
    );
    f.kernel
        .resolve_obligation(obligation, &mut f.meter)
        .unwrap();
    agree(f.target, &mut f);
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
    agree(f.target, &mut f);
    let owner_scope = f.kernel.job_scope::<Stop>(f.owner).unwrap();
    agree(owner_scope, &mut f);
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    assert_eq!(
        run.current_generation(&f.entry).unwrap().state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    agree(owner_scope, &mut f);
    assert!(
        matches!(f.kernel.closure_in_metered(owner_scope, &mut f.meter).unwrap(), Closure::Open(o) if o.missing > 0)
    );
}

#[test]
fn obligation_indexes_resolve_retire_and_fork_without_history_queries() {
    let mut f = Fixture::new();
    let obligation = f.kernel.obligation(f.target, &mut f.meter).unwrap();
    let mut fork = f.kernel.fork(&mut f.meter).unwrap();
    fork.kernel()
        .resolve_obligation(obligation, &mut f.meter)
        .unwrap();
    assert_eq!(
        fork.view()
            .closure_in_metered(f.target, &mut f.meter)
            .unwrap(),
        Closure::Closed
    );
    assert!(
        matches!(f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap(), Closure::Open(o) if o.obligations == 1)
    );
    drop(fork);
    f.kernel
        .resolve_obligation(obligation, &mut f.meter)
        .unwrap();
    assert_eq!(
        f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap(),
        Closure::Closed
    );
    let again = f.kernel.obligation(f.target, &mut f.meter).unwrap();
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    run.withdraw_generation(&mut f.entry).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        f.kernel.resolve_obligation(again, &mut f.meter),
        Err(KernelError::InactiveScope)
    );
    let owner = f.kernel.job_scope::<Stop>(f.owner).unwrap();
    agree(owner, &mut f);
}

#[test]
fn local_quote_is_independent_of_thousands_of_unrelated_retired_records() {
    fn quote(history: u64) -> usize {
        let mut f = Fixture::new();
        for index in 0..history {
            let old = scope(&mut f.kernel, &mut f.meter, 50, index);
            let job = f.kernel.register(old, (), &mut f.meter).unwrap();
            let obligation = f.kernel.obligation(old, &mut f.meter).unwrap();
            f.kernel.slot::<u8, _>(old, &mut f.meter).unwrap();
            f.kernel
                .begin(job, &mut f.meter)
                .unwrap()
                .finish(LocalClosure::Stable)
                .unwrap();
            f.kernel
                .resolve_obligation(obligation, &mut f.meter)
                .unwrap();
            f.kernel.retire_scope(old, &mut f.meter).unwrap();
        }
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.query(&mut f.inputs, Scoped::new(f.source, 42)).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        f.kernel.obligation(f.target, &mut f.meter).unwrap();
        let before = f.meter.used;
        assert!(
            matches!(f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap(), Closure::Open(o) if o.obligations == 1 && o.missing == 1)
        );
        f.meter.used - before
    }
    assert_eq!(quote(0), quote(10_000));
}

struct CancelAfter<'a> {
    root: &'a mut Budget,
    at: usize,
    cancelled: Cell<bool>,
}
impl Meter for CancelAfter<'_> {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.root.reserve(work)?;
        if self.root.calls == self.at {
            self.cancelled.set(true);
        }
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled.get() {
            Err(Stop::Cancelled)
        } else {
            self.root.checkpoint()
        }
    }
}

#[test]
fn every_readiness_refusal_and_cancellation_preserves_graph_and_pending_work() {
    let phases = {
        let mut f = Fixture::new();
        let before = f.meter.calls;
        f.kernel.closure_in_metered(f.target, &mut f.meter).unwrap();
        f.meter.calls - before
    };
    for phase in 1..=phases {
        let mut f = Fixture::new();
        let before = f.kernel.closure();
        let stats = f.kernel.statistics();
        let queue = f.kernel.worklist().clone();
        f.meter.fail_on = Some(f.meter.calls + phase);
        assert_eq!(
            f.kernel.closure_in_metered(f.target, &mut f.meter),
            Err(KernelError::Stopped(Stop::Work))
        );
        f.meter.fail_on = None;
        {
            let at = f.meter.calls + phase;
            let mut meter = CancelAfter {
                root: &mut f.meter,
                at,
                cancelled: Cell::new(false),
            };
            assert_eq!(
                f.kernel.closure_in_metered(f.target, &mut meter),
                Err(KernelError::Stopped(Stop::Cancelled))
            );
        }
        assert_eq!(f.kernel.closure(), before);
        assert_eq!(f.kernel.statistics(), stats);
        assert_eq!(f.kernel.worklist(), &queue);
        agree(f.target, &mut f);
    }
}
