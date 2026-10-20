use super::Stop;
use crate::{
    Analyzer, Closure, Kernel, KernelError, LocalClosure, Meter, Nil, ScopeKey, SolveError, solve,
};
use std::{cell::Cell, convert::Infallible};

#[derive(Default)]
struct Ledger {
    calls: usize,
    used: usize,
    refuse: bool,
    fail_on: Option<usize>,
    checkpoints: Cell<usize>,
    cancel_at: Option<usize>,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        self.calls += 1;
        if self.refuse || self.fail_on == Some(self.calls) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(cost).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        self.checkpoints.set(self.checkpoints.get() + 1);
        if self
            .cancel_at
            .is_some_and(|at| self.checkpoints.get() >= at)
        {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

struct Noop;
impl<Cx> Analyzer<Cx> for Noop {
    type Task = ();
    type Error = Infallible;
    fn evaluate(&mut self, _: &(), _: &mut Cx) -> Result<LocalClosure, Infallible> {
        Ok(LocalClosure::Stable)
    }
}

fn closed_kernel() -> (Kernel<()>, Ledger) {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = kernel
        .scope(
            ScopeKey {
                context: 1,
                generation: 0,
                policy: 1,
            },
            &mut meter,
        )
        .unwrap();
    for _ in 0..4096 {
        kernel.slot::<u8, _>(scope, &mut meter).unwrap();
    }
    kernel.seal_scope(scope, &mut meter).unwrap();
    (kernel, meter)
}

#[test]
fn public_4096_slot_probe_cannot_return_reports_on_a_refusing_meter() {
    let (mut kernel, mut meter) = closed_kernel();
    let used = meter.used;
    meter.refuse = true;
    for max_steps in [0, 1, 1] {
        let calls = meter.calls;
        assert_eq!(
            solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, max_steps),
            Err(SolveError::Kernel(KernelError::Stopped(Stop::Work)))
        );
        assert!(
            meter.calls > calls,
            "even an empty ready queue must reserve report work"
        );
        assert_eq!(
            meter.used, used,
            "the cumulative ledger is neither reset nor refunded"
        );
    }
}

#[test]
fn zero_step_and_empty_queue_reports_check_cancellation() {
    let (mut kernel, mut meter) = closed_kernel();
    let before = (meter.calls, meter.used);
    meter.cancel_at = Some(meter.checkpoints.get() + 1);
    for max_steps in [0, 1, 1] {
        let checks = meter.checkpoints.get();
        assert_eq!(
            solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, max_steps),
            Err(SolveError::Kernel(KernelError::Stopped(Stop::Cancelled)))
        );
        assert!(meter.checkpoints.get() > checks);
        assert_eq!(
            (meter.calls, meter.used),
            before,
            "reject before scanning graph metadata"
        );
    }
}

#[test]
fn every_successful_report_prepays_traversal_on_the_same_root_ledger() {
    let (mut kernel, mut meter) = closed_kernel();
    for max_steps in [0, 1, 1] {
        let before = (meter.calls, meter.used);
        let report = solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, max_steps).unwrap();
        assert_eq!(report.closure, Closure::Closed);
        assert_eq!(report.steps, 0);
        assert!(!report.local_limits);
        assert!(
            meter.calls >= before.0 + 2,
            "quote scan and full traversal are prepaid separately"
        );
        assert!(
            meter.used > before.1 + 4096,
            "retained facts contribute even without ready jobs"
        );
    }
}

#[test]
fn refusal_after_paid_metadata_quotation_cannot_return_an_uncharged_report() {
    let (mut kernel, mut meter) = closed_kernel();
    let used = meter.used;
    meter.fail_on = Some(meter.calls + 2);
    assert_eq!(
        solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0),
        Err(SolveError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    assert!(
        meter.used > used,
        "the successful quotation pass remains charged"
    );
    meter.fail_on = None;
    let paid = meter.used;
    assert_eq!(
        solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0)
            .unwrap()
            .closure,
        Closure::Closed
    );
    assert!(meter.used > paid);
}

#[test]
fn cancellation_during_quotation_or_final_reporting_never_returns_success() {
    let (mut kernel, mut meter) = closed_kernel();
    for checkpoint in [2, 3] {
        let used = meter.used;
        meter.cancel_at = Some(meter.checkpoints.get() + checkpoint);
        assert_eq!(
            solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, 0),
            Err(SolveError::Kernel(KernelError::Stopped(Stop::Cancelled)))
        );
        assert!(meter.used > used, "already performed work is not refunded");
        meter.cancel_at = None;
    }
}
