//! Phase registration owns analyzer state and both kinds of store factories.
use crate::{native, support};
use analysis_kernel::{
    Analyzer, Kernel, KernelError, LocalClosure, Meter, Read, ReclaimScope, Scope, Scoped,
};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, HybridConfig, MachineError,
        Status,
        control::Control,
        cooperation::{
            CooperationError, Extension, ExtraStores, InputRoute, InputView, Inputs, Subject,
        },
    },
    domain::NumericValue,
    world::EvmByteArray,
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Counts {
    local: Cell<usize>,
    shared: Cell<usize>,
    dropped: Cell<usize>,
    calls: Cell<usize>,
    process: Cell<usize>,
}
struct Store(Rc<Counts>);
impl Drop for Store {
    fn drop(&mut self) {
        self.0.dropped.set(self.0.dropped.get() + 1);
    }
}
impl ReclaimScope for Store {
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        _: &mut Kernel<J>,
        _: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        meter.reserve(1).map_err(KernelError::Stopped)?;
        Ok(0)
    }
}
struct Factory {
    counts: Rc<Counts>,
    shared: bool,
}
impl ExtraStores for Factory {
    type Stores = Store;
    fn retirement_work(&self, _: &Store) -> usize {
        1
    }
    fn create<J, M: Meter>(
        &self,
        _: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Store, KernelError<M::Stop>> {
        meter.reserve(8).map_err(KernelError::Stopped)?;
        let count = if self.shared {
            &self.counts.shared
        } else {
            &self.counts.local
        };
        count.set(count.get() + 1);
        Ok(Store(Rc::clone(&self.counts)))
    }
}
#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Kernel(#[from] KernelError<MachineError>),
    #[error("native sentinel")]
    Sentinel,
}
struct Watch {
    counts: Rc<Counts>,
    fail: bool,
}
impl<Cx: Read<Inputs<NumericValue>, InputRoute, Error = KernelError<MachineError>>> Analyzer<Cx>
    for Watch
{
    type Task = Subject;
    type Error = Error;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Error> {
        let input = cx.query(Scoped::new(subject.scope, ()))?;
        if let Some(input) = input.value() {
            self.counts.calls.set(self.counts.calls.get() + 1);
            if matches!(input.view(), InputView::Process(_)) {
                self.counts.process.set(self.counts.process.get() + 1);
            }
            if self.fail {
                return Err(Error::Sentinel);
            }
        }
        Ok(LocalClosure::Stable)
    }
}
fn source() -> AnalysisInput<NumericValue> {
    let mut world = support::new_world();
    let fixture = support::Fixture {
        bytes: vec![revm_bytecode::opcode::STOP],
        labels: Default::default(),
    };
    support::insert(&mut world, support::A, &fixture, &[(0, 7)]);
    AnalysisInput::new(world, support::entry(EvmByteArray::empty())).unwrap()
}
fn extension(
    counts: &Rc<Counts>,
    fail: bool,
) -> Extension<Watch, evm_abstract::analysis::cooperation::PerInput, Factory, Factory> {
    Extension::new(Watch {
        counts: Rc::clone(counts),
        fail,
    })
    .with_stores(Factory {
        counts: Rc::clone(counts),
        shared: false,
    })
    .with_shared_stores(Factory {
        counts: Rc::clone(counts),
        shared: true,
    })
}
fn config() -> ExecutionConfig {
    ExecutionConfig {
        root_terminal_coverage: false,
        ..Default::default()
    }
}
fn hybrid(detail: usize) -> HybridConfig {
    HybridConfig {
        max_detail_work: detail,
        ..Default::default()
    }
}

#[test]
fn native_only_is_explicit_and_uses_only_detail_stores_with_one_cumulative_ledger() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false).native_only();
    let values = native::product();
    let policies = EnginePolicies::default();
    let input = source();
    let original = input.entry().clone();
    let result = AnalysisEngine::new(&values, &policies, &mut extension)
        .run_hybrid(input, config(), hybrid(2_000_000), &Control::default())
        .unwrap();
    assert!(result.coverage().baseline_current && result.coverage().detail_current);
    assert_eq!(result.status(), Status::Converged);
    assert_eq!(result.baseline().entry(), &original);
    assert_eq!(result.detail().unwrap().entry(), &original);
    assert!(counts.calls.get() > 0 && counts.local.get() > 0);
    assert_eq!(counts.process.get(), 0);
    assert_eq!(
        counts.shared.get(),
        1,
        "only detail owns the client global store"
    );
    assert_eq!(
        counts.dropped.get(),
        counts.local.get() + counts.shared.get()
    );
    let work = result.coverage().work;
    assert_eq!(
        result.work(),
        work.prior + work.baseline + work.preparation + work.detail
    );
    assert!(result.work() <= config().max_work);
    assert_eq!(result.detail().unwrap().work(), result.work());
}
#[test]
fn disabled_native_detail_does_not_initialize_or_call_an_explicit_native_extension() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false).native_only();
    let values = native::product();
    let policies = EnginePolicies::default();
    let result = AnalysisEngine::new(&values, &policies, &mut extension)
        .run_hybrid(source(), config(), hybrid(0), &Control::default())
        .unwrap();
    assert!(result.coverage().baseline_current);
    assert!(result.detail().is_none());
    assert_eq!(
        (counts.calls.get(), counts.local.get(), counts.shared.get()),
        (0, 0, 0)
    );
}
#[test]
fn an_unselected_phase_contract_fails_before_allocating_client_stores() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false);
    let values = native::product();
    let policies = EnginePolicies::default();
    let result = AnalysisEngine::new(&values, &policies, &mut extension).run_hybrid(
        source(),
        config(),
        hybrid(2_000_000),
        &Control::default(),
    );
    assert!(matches!(
        result,
        Err(CooperationError::UnsupportedView { .. })
    ));
    assert_eq!(
        (counts.calls.get(), counts.local.get(), counts.shared.get()),
        (0, 0, 0)
    );
}
#[test]
fn native_analyzer_error_survives_the_phase_boundary_and_disposes_both_stores() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, true).native_only();
    let values = native::product();
    let policies = EnginePolicies::default();
    let result = AnalysisEngine::new(&values, &policies, &mut extension).run_hybrid(
        source(),
        config(),
        hybrid(2_000_000),
        &Control::default(),
    );
    assert!(matches!(
        result,
        Err(CooperationError::Extension(Error::Sentinel))
    ));
    assert!(counts.calls.get() > 0);
    assert_eq!(counts.shared.get(), 1);
    assert_eq!(
        counts.dropped.get(),
        counts.local.get() + counts.shared.get()
    );
}
#[test]
fn process_aware_all_phase_contract_recreates_global_and_local_stores_per_kernel() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false).with_process_view();
    let values = native::product();
    let policies = EnginePolicies::default();
    let result = AnalysisEngine::new(&values, &policies, &mut extension)
        .run_hybrid(source(), config(), hybrid(2_000_000), &Control::default())
        .unwrap();
    assert!(result.coverage().baseline_current && result.coverage().detail_current);
    assert!(counts.process.get() > 0);
    assert_eq!(counts.shared.get(), 2);
    assert_eq!(
        counts.dropped.get(),
        counts.local.get() + counts.shared.get()
    );
}
#[test]
fn a_tiny_detail_allowance_keeps_the_current_process_baseline() {
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false).native_only();
    let values = native::product();
    let policies = EnginePolicies::default();
    let result = AnalysisEngine::new(&values, &policies, &mut extension)
        .run_hybrid(source(), config(), hybrid(1), &Control::default())
        .unwrap();
    assert!(result.coverage().baseline_current);
    assert_eq!(counts.calls.get(), 0);
    assert!(result.work() <= config().max_work);
}

#[test]
fn native_only_registration_does_not_bypass_a_direct_process_view_requirement() {
    use evm_abstract::analysis::{
        BoundedJoin, ContextInsensitive, FeedbackAcceleration, KnownProcessSummaries,
        MergeResumeStatus, SemanticPolicies, StructuralPartition,
    };
    let counts = Rc::new(Counts::default());
    let mut extension = extension(&counts, false).native_only();
    let values = native::product();
    let policies = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            KnownProcessSummaries,
        ),
    );
    let result = AnalysisEngine::new(&values, &policies, &mut extension).run(
        source(),
        config(),
        &Control::default(),
    );
    assert!(matches!(
        result,
        Err(CooperationError::UnsupportedView { .. })
    ));
    assert_eq!(
        (counts.calls.get(), counts.local.get(), counts.shared.get()),
        (0, 0, 0)
    );
}
