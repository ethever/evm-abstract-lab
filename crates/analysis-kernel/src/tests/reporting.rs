use super::{Noop, Stop};
use crate::{Closure, Kernel, KernelError, Meter, Nil, ScopeKey, SolveError, solve};
use std::cell::Cell;

#[derive(Default)]
struct Ledger {
    calls: usize,
    used: usize,
    fail_on: Option<usize>,
    checkpoints: Cell<usize>,
    cancel_at: Option<usize>,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        self.calls += 1;
        if self.fail_on == Some(self.calls) {
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
fn every_report_payment_refusal_retains_root_charges_and_can_retry() {
    for phase in [1, 2] {
        let (mut kernel, mut meter) = closed_kernel();
        for max_steps in [0, 1, 1] {
            let (calls, used) = (meter.calls, meter.used);
            meter.fail_on = Some(calls + phase);
            assert_eq!(
                solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, max_steps),
                Err(SolveError::Kernel(KernelError::Stopped(Stop::Work)))
            );
            assert_eq!(meter.calls, calls + phase);
            if phase == 1 {
                assert_eq!(meter.used, used, "no failed payment is charged");
            } else {
                assert!(meter.used > used, "the paid quotation is not refunded");
            }
        }
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
}

#[test]
fn every_report_cancellation_boundary_retains_exactly_the_already_paid_work() {
    let (mut kernel, mut meter) = closed_kernel();
    // Repeated zero-step/empty-queue attempts and both paid-report checkpoints.
    for (max_steps, checkpoint) in [(0, 1), (1, 1), (1, 1), (0, 2), (0, 3)] {
        let before = (meter.calls, meter.used);
        let checks = meter.checkpoints.get();
        meter.cancel_at = Some(checks + checkpoint);
        assert_eq!(
            solve(&mut kernel, &mut Noop, &mut Nil, &mut meter, max_steps),
            Err(SolveError::Kernel(KernelError::Stopped(Stop::Cancelled)))
        );
        assert_eq!(meter.checkpoints.get(), checks + checkpoint);
        if checkpoint == 1 {
            assert_eq!(
                (meter.calls, meter.used),
                before,
                "reject before graph scans"
            );
        } else {
            assert!(meter.used > before.1, "already paid work is not refunded");
        }
        meter.cancel_at = None;
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
