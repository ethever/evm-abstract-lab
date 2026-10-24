//! Real enabled input views and same-stamp dependency replacement own each copy.

use super::super::Cooperation;
use crate::{
    U256,
    analysis::{
        EvmMachinePayload, MachineError,
        cooperation::{
            CooperationError, Extension, InputRoute, Inputs, LocalInput, Point, PointKind, Subject,
        },
        worklist::{
            MachineMeter,
            runtime::{Runtime, TaskKind},
        },
    },
    domain::{EvmValue, ValueAlgebra},
    resource::{
        WorkBudget,
        clone_payment::tests::{RECEIPT, Receipt, SummaryPolicy, Word, reset, summary_fixture},
    },
};
use analysis_kernel::{
    Analyzer, Change, Closure, KernelError, LocalClosure, Read, RunningJob, Scoped, SlotMeta,
    Version,
};
use std::{cell::Cell, rc::Rc};

struct Reader(Rc<Cell<usize>>);
impl<Cx: Read<Inputs<Word>, InputRoute>> Analyzer<Cx> for Reader {
    type Task = Subject;
    type Error = Cx::Error;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        assert!(cx.query(Scoped::new(subject.scope, ()))?.value().is_some());
        self.0.set(self.0.get() + 1);
        Ok(LocalClosure::Stable)
    }
}

struct Harness {
    cooperation: Cooperation<Word, Extension<Reader>, KernelError<MachineError>>,
    runtime: Runtime<Subject>,
    owner: RunningJob,
    anchor: SlotMeta<()>,
    assumption: SlotMeta<()>,
    version: Version,
    algebra: ValueAlgebra<SummaryPolicy>,
    read: Rc<Cell<usize>>,
}
impl Harness {
    const POINT: Point = Point {
        namespace: 1,
        kind: PointKind::Body,
        index: 0,
    };

    fn new(word: Word) -> (Self, EvmMachinePayload<Word>) {
        let (world, entry, _, algebra) = summary_fixture("00");
        let mut payload =
            crate::analysis::transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
        payload
            .active_mut()
            .stack
            .push(EvmValue::from_numeric(word));
        payload.normalize();
        // Setup creates the real owner and slots; the tested ledger begins only
        // after this owned payload has been prepared. No native copy is needed.
        let mut setup = WorkBudget::new(usize::MAX);
        let mut runtime = Runtime::new(&mut setup).unwrap();
        let relative = runtime.relative;
        let anchor = runtime
            .kernel()
            .slot(relative, &mut MachineMeter(&mut setup))
            .unwrap();
        let assumption = runtime
            .kernel()
            .slot(relative, &mut MachineMeter(&mut setup))
            .unwrap();
        runtime
            .register_relative(TaskKind::Body(0), &mut setup)
            .unwrap();
        let (_, mut owner) = runtime.next(true, &mut setup).unwrap().unwrap();
        let version = runtime
            .with_run(&mut owner, &mut setup, |run| {
                run.slot_version(&anchor)
                    .map_err(crate::analysis::worklist::kernel_failure)
            })
            .unwrap();
        let read = Rc::new(Cell::new(0));
        let mut cooperation = Cooperation::new(Extension::new(Reader(Rc::clone(&read))));
        cooperation
            .begin_owner(&mut runtime, &mut owner, &mut setup)
            .unwrap();
        (
            Self {
                cooperation,
                runtime,
                owner,
                anchor,
                assumption,
                version,
                algebra,
                read,
            },
            payload,
        )
    }

    fn input(
        &mut self,
        payload: EvmMachinePayload<Word>,
        budget: &mut WorkBudget,
    ) -> Result<LocalInput<Word>, CooperationError<KernelError<MachineError>>> {
        let anchor = &self.anchor;
        let assumption = &self.assumption;
        let version = self.version;
        self.cooperation.input(
            Self::POINT,
            version,
            0,
            None,
            None,
            payload,
            self.algebra.borrowed(),
            &mut self.runtime,
            &mut self.owner,
            budget,
            |run| {
                assert_eq!(run.observe_slot(anchor)?, version);
                run.observe_slot(assumption)?;
                Ok(())
            },
        )
    }

    fn reseed_payload(&mut self) -> EvmMachinePayload<Word> {
        let payload = self.cooperation.bindings[&Self::POINT].input.payload();
        let mut setup = WorkBudget::new(usize::MAX);
        payload.pay_clone_with(&mut setup).unwrap();
        let payload = payload.clone();
        self.runtime
            .with_run(&mut self.owner, &mut setup, |run| {
                run.publish_prepared_with(&self.assumption, Change::Changed, 1, || {})
                    .map_err(crate::analysis::worklist::kernel_failure)
            })
            .unwrap();
        assert!(
            self.runtime
                .view()
                .generation_stale(
                    &self.cooperation.bindings[&Self::POINT].entry,
                    &mut MachineMeter(&mut setup)
                )
                .unwrap()
        );
        payload
    }
}

#[test]
fn enabled_view_charges_its_native_copy_and_preserves_private_input() {
    let _scope = RECEIPT.scope();
    let mut costs = Vec::new();
    for fee in [64, 1_000_000] {
        let (mut h, payload) = Harness::new(Word::expensive(fee));
        let mut budget = WorkBudget::new(100_000_000);
        reset();
        let LocalInput::Reachable { payload, .. } = h.input(payload, &mut budget).unwrap() else {
            panic!("ready input")
        };
        let receipt = RECEIPT.get();
        assert_eq!(
            receipt.bytes, 4096,
            "exactly the detached native view is copied"
        );
        assert_eq!(receipt.private_quotes, 1);
        assert_eq!(h.read.get(), 1, "the real enabled provider ran");
        assert!(!payload.active().stack[0].contains(U256::from(7)));
        assert!(payload.active().stack[0].contains(U256::from(8)));
        assert!(!budget.exhausted());
        costs.push(budget.used());
    }
    assert_eq!(costs[1] - costs[0], 1_000_000 - 64);
}

#[test]
fn enabled_view_refuses_finite_and_overflow_copy_fees_before_native_clone() {
    let _scope = RECEIPT.scope();
    for fee in [1_000_000, usize::MAX] {
        let (mut h, payload) = Harness::new(Word::expensive(fee));
        let mut budget = WorkBudget::new(500_000);
        reset();
        let error = h.input(payload, &mut budget).expect_err("copy must refuse");
        assert!(matches!(
            error,
            CooperationError::Protocol(KernelError::Stopped(MachineError::Work))
        ));
        assert_eq!(RECEIPT.get().bytes, 0);
        assert!(
            RECEIPT.get().private_quotes > 0,
            "reached the native view invoice"
        );
        assert_eq!(
            h.read.get(),
            1,
            "provider closure preceded this refused copy"
        );
        assert_ne!(
            h.runtime.view().closure(),
            Closure::Closed,
            "native owner stays unfinished"
        );
        assert_eq!(h.cooperation.stop(error, &budget), MachineError::Work);
        assert!(h.cooperation.interrupted);
        assert!(
            h.cooperation.subject(Harness::POINT).is_some(),
            "immutable source remains retained"
        );
    }
}

#[test]
fn same_stamp_reseed_pays_before_copy_and_keeps_old_generation_on_refusal() {
    let _scope = RECEIPT.scope();
    let (mut h, payload) = Harness::new(Word::expensive(1_000_000));
    h.input(payload, &mut WorkBudget::new(100_000_000)).unwrap();
    let old = h.cooperation.subject(Harness::POINT).unwrap();
    let mut request = h.reseed_payload();
    // An unchanged native stamp reuses the retained snapshot, including its
    // independent price, even if this caller supplies a cheaper detached view.
    request.active_mut().stack[0] = EvmValue::constant(U256::from(9));
    let mut budget = WorkBudget::new(500_000);
    reset();
    let error = h
        .input(request, &mut budget)
        .expect_err("reseed must refuse");
    assert!(matches!(
        error,
        CooperationError::Protocol(KernelError::Stopped(MachineError::Work))
    ));
    assert_eq!(RECEIPT.get().bytes, 0);
    assert!(
        RECEIPT.get().private_quotes > 0,
        "same-stamp reseed reached its own copy invoice"
    );
    assert_eq!(
        h.read.get(),
        1,
        "replacement cannot publish before its snapshot is ready"
    );
    assert_eq!(h.cooperation.subject(Harness::POINT), Some(old));
    assert_ne!(h.runtime.view().closure(), Closure::Closed);
    // Retrying the same immutable source with an affordable ledger creates a
    // fresh scope; reseeding and the subsequent view each pay their own copy.
    let mut request = h.reseed_payload();
    request.active_mut().stack[0] = EvmValue::constant(U256::from(9));
    reset();
    let LocalInput::Reachable { payload, .. } =
        h.input(request, &mut WorkBudget::new(100_000_000)).unwrap()
    else {
        panic!("fresh dependency scope is ready")
    };
    assert_eq!(RECEIPT.get().bytes, 8192);
    assert_eq!(RECEIPT.get().private_quotes, 2);
    assert_ne!(h.cooperation.subject(Harness::POINT), Some(old));
    assert!(!payload.active().stack[0].contains(U256::from(7)));
    assert!(payload.active().stack[0].contains(U256::from(8)));
}

#[test]
fn view_invoice_honors_both_cancellation_owners_before_native_clone() {
    let _scope = RECEIPT.scope();
    for captured_target in [true, false] {
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut word = Word::expensive(64);
        word.cancel_price = Some(if captured_target {
            captured.clone()
        } else {
            ambient.clone()
        });
        let (mut h, payload) = Harness::new(word);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000_000));
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        let error = analysis_control::with_cancellation(&ambient, || h.input(payload, &mut budget))
            .expect_err("quote-triggered cancellation must refuse");
        assert!(matches!(
            error,
            CooperationError::Protocol(KernelError::Stopped(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            )))
        ));
        assert_eq!(RECEIPT.get().bytes, 0);
        assert!(RECEIPT.get().private_quotes > 0);
        assert_eq!(h.read.get(), 1);
        assert_ne!(h.runtime.view().closure(), Closure::Closed);
    }
}
