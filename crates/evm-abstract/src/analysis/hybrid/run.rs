//! Both phases share their caller's native ledger and allocation counters.

use crate::analysis::{
    AnalysisEngine, AnalysisInput, BoundedJoin, ContextInsensitive, ContextPolicy, EnginePolicies,
    EvmWorldAnalysis, ExecutionConfig, FeedbackAcceleration, KnownProcessSummaries,
    MergeResumeStatus, PartitionPolicy, ProjectionError, RuntimeExecution, SemanticPolicies,
    StatePolicy, Status, StructuralPartition,
    control::Control,
    cooperation::{Composition, CooperationError, NoExtension, PhaseScope},
    engine::{
        self,
        cooperation::{Borrowed, Cooperation},
    },
    publication::{ModelHandle, NoopPublisher, StagePublisher},
    transfer::StorageReadPolicy,
};
use crate::{
    domain::{EvmNumericAlgebra, WordElement},
    resource::{CloneError, WorkBudget},
    world::RootInputBinding,
};

use super::{
    DetailState, HybridAnalysis, HybridConfig, HybridCoverage, HybridInterpretation,
    HybridNativeBinding, HybridWork, input, local, policy,
};

type HybridResult<N, E> = Result<HybridAnalysis<N>, CooperationError<E>>;

/// Source currency is already computed by the kernel adapter. Fund just this
/// bounded receipt walk; failure retains the actual baseline/detail status and
/// withholds the additional positive certificate.
fn current<N: WordElement>(report: &EvmWorldAnalysis<N>, budget: &mut WorkBudget) -> bool {
    if report.status() != Status::Converged || !report.frontiers().is_empty() {
        return false;
    }
    let work = report
        .procedure_summaries()
        .len()
        .saturating_mul((usize::BITS as usize).saturating_add(1))
        .saturating_mul(4)
        .saturating_add(1);
    if !budget.can_afford(work) {
        return false;
    }
    if !budget.charge(work) {
        return false;
    }
    // Older records are historical observations after the same World input
    // grows. Only its latest record can certify that source's current transfer.
    let mut sources = std::collections::BTreeSet::new();
    report.procedure_summaries().iter().rev().all(|record| {
        !sources.insert(record.source_state) || (record.closed && record.source_current)
    })
}

impl<D, C, P, S, X> AnalysisEngine<'_, D, C, P, S, X>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    /// Close conservative global process control before spending any optional
    /// precision. Existing `run` keeps the caller's ordinary precise behavior.
    /// Requested terminal coverage belongs only to optional native detail;
    /// it never certifies process closure or whole-native completion.
    pub fn run_hybrid(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        hybrid: HybridConfig,
        control: &Control,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        self.run_hybrid_with_publisher(input, config, hybrid, control, &mut NoopPublisher)
    }

    /// Publish the closed global model before starting optional Native precision.
    /// Cancellation after publication leaves retained immutable handles usable.
    pub fn run_hybrid_with_publisher<T: StagePublisher<D::Element>>(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        hybrid: HybridConfig,
        control: &Control,
        publisher: &mut T,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        control.scope(|| {
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            let mut budget = WorkBudget::new(config.max_work);
            let mut counters = engine::Counters::with_observer(control.observer());
            let report = self.run_hybrid_metered_with_publisher(
                input,
                config,
                hybrid,
                (&mut budget, &mut counters),
                StorageReadPolicy::Abstract,
                publisher,
            )?;
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            Ok(report)
        })
    }

    /// Session/RPC adapters must retain their original declared ledger and
    /// counters across source restarts. Baseline reads remain Abstract; the
    /// supplied read policy applies only to native precision discovery.
    /// Never call public run_hybrid as a rescue
    /// after an earlier run or sticky refusal. No kernel graph is rehydrated.
    #[cfg(test)]
    pub(in crate::analysis) fn run_hybrid_metered(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        hybrid: HybridConfig,
        budget: &mut WorkBudget,
        counters: &mut engine::Counters,
        detail_reads: StorageReadPolicy,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        self.run_hybrid_metered_with_publisher(
            input,
            config,
            hybrid,
            (budget, counters),
            detail_reads,
            &mut NoopPublisher,
        )
    }

    pub(in crate::analysis) fn run_hybrid_metered_with_publisher<T: StagePublisher<D::Element>>(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        hybrid: HybridConfig,
        payer: (&mut WorkBudget, &mut engine::Counters),
        detail_reads: StorageReadPolicy,
        publisher: &mut T,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        let (budget, counters) = payer;
        crate::resource::clone_payment::checkpoint(budget).map_err(|error| match error {
            CloneError::Cancelled => CooperationError::Cancelled,
            CloneError::Work => CooperationError::Protocol(analysis_kernel::KernelError::Stopped(
                crate::analysis::MachineError::work_failure(),
            )),
        })?;
        let prior = budget.used();
        // Reserve the immutable graph handle before execution can exhaust work.
        // Publication never obtains a fresh ledger after Native refusal.
        crate::resource::clone_payment::pay(budget, 1).map_err(|error| match error {
            CloneError::Cancelled => CooperationError::Cancelled,
            CloneError::Work => CooperationError::Protocol(analysis_kernel::KernelError::Stopped(
                crate::analysis::MachineError::work_failure(),
            )),
        })?;
        let AnalysisInput {
            world,
            entry,
            facts,
        } = input;
        let coarse = EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                BoundedJoin,
                FeedbackAcceleration,
                KnownProcessSummaries,
            ),
        );
        let mut baseline_config = config.clone();
        baseline_config.root_terminal_coverage = false;
        let mut baseline = if self.extension.phase_scope() == PhaseScope::NativeOnly {
            engine::run_reusing(
                world,
                entry,
                baseline_config,
                self.algebra.borrowed(),
                &coarse,
                budget,
                counters,
                StorageReadPolicy::Abstract,
                None,
                (facts.snapshot(), facts.revision()),
                Cooperation::new(NoExtension),
            )
            .map_err(|error| error.map_extension(|never| match never {}))?
        } else {
            engine::run_reusing(
                world,
                entry,
                baseline_config,
                self.algebra.borrowed(),
                &coarse,
                budget,
                counters,
                StorageReadPolicy::Abstract,
                None,
                (facts.snapshot(), facts.revision()),
                Cooperation::new(Borrowed(&mut *self.extension)),
            )?
        };
        let baseline_current = current(&baseline, budget);
        baseline.work = budget.used();
        let coverage = HybridCoverage {
            baseline_current,
            detail_current: false,
            detail_binding: match baseline.root_binding() {
                RootInputBinding::Physical { .. } => HybridNativeBinding::OriginalOwner,
                RootInputBinding::Namespace { coordinate } => {
                    HybridNativeBinding::SymbolicOwnerNamespace {
                        namespace: coordinate,
                    }
                }
            },
            local_detail_sites: 0,
            detail_regions: Vec::new(),
            chosen: HybridInterpretation::Process,
            detail: if !baseline_current {
                DetailState::BaselinePending
            } else if hybrid.max_detail_work == 0 || hybrid.max_detail_regions == 0 {
                DetailState::Disabled
            } else {
                DetailState::Pending
            },
            requested_detail_work: hybrid.max_detail_work,
            requested_native_terminal_coverage: config.root_terminal_coverage,
            work: HybridWork {
                prior,
                baseline: budget.used().saturating_sub(prior),
                total: budget.used(),
                ..HybridWork::default()
            },
        };
        let baseline = ModelHandle::new(baseline);
        let staged = HybridAnalysis {
            baseline,
            detail: None,
            coverage,
        };
        if staged.coverage.baseline_current {
            publisher.publish(&staged);
            publisher
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
        }
        let HybridAnalysis {
            baseline,
            mut coverage,
            ..
        } = staged;
        let mut detail = None;
        if hybrid.max_detail_work == 0 || hybrid.max_detail_regions == 0 {
            coverage.detail = DetailState::Disabled;
        } else if baseline_current && budget.can_afford(0) {
            if self.policies.state.runtime_execution() == RuntimeExecution::Process {
                coverage.detail = DetailState::ProcessPolicy;
            } else {
                let allowance = hybrid.max_detail_work.min(budget.remaining());
                coverage.work.detail_allowance = allowance;
                let start = budget.used();
                let result = budget.with_allowance(allowance, |budget| {
                    // The same cap must survive later RPC acquisition/restart;
                    // the temporary call boundary never reopens this allowance.
                    match budget.limit_remaining(allowance) {
                        Ok(()) => {}
                        Err(CloneError::Work) => {
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                        Err(CloneError::Cancelled) => return Err(CooperationError::Cancelled),
                    }
                    // This bounded metadata walk may publish after a later
                    // detail refusal. Its original-ledger invoice is paid
                    // now, not rescued or charged to a fresh ledger later.
                    let metadata = match budget.try_completion_escrow(
                        config
                            .analysis
                            .max_states
                            .saturating_mul(4)
                            .saturating_add(1),
                    ) {
                        Ok(permit) => permit,
                        Err(_) => {
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                    };
                    let source = input::copy(baseline.world(), baseline.entry(), budget);
                    coverage.work.preparation = budget.used().saturating_sub(start);
                    let (world, mut entry) = match source {
                        Ok(source) => source,
                        Err(CloneError::Work) => {
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                        Err(CloneError::Cancelled) => return Err(CooperationError::Cancelled),
                    };
                    if let HybridNativeBinding::SymbolicOwnerNamespace { namespace } =
                        coverage.detail_binding
                    {
                        // Materialize the explicit subset condition on the paid
                        // detached native entry. The original symbolic entry is
                        // still retained by the complete process baseline.
                        match crate::resource::clone_payment::pay(budget, 1) {
                            Ok(()) => entry.environment.to = namespace.into(),
                            Err(CloneError::Work) => {
                                coverage.work.preparation = budget.used().saturating_sub(start);
                                coverage.detail = DetailState::Work;
                                return Ok(None);
                            }
                            Err(CloneError::Cancelled) => return Err(CooperationError::Cancelled),
                        }
                    }
                    coverage.detail_regions = match policy::select(
                        &baseline,
                        &entry,
                        hybrid.max_detail_regions,
                        budget,
                    ) {
                        Ok(regions) => regions,
                        Err(ProjectionError::Work) => {
                            coverage.work.preparation = budget.used().saturating_sub(start);
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                        Err(ProjectionError::Cancelled) => return Err(CooperationError::Cancelled),
                        Err(ProjectionError::MissingProgram | ProjectionError::NonRuntime(_)) => {
                            coverage.work.preparation = budget.used().saturating_sub(start);
                            coverage.detail = DetailState::Incomplete;
                            return Ok(None);
                        }
                    };
                    coverage.work.preparation = budget.used().saturating_sub(start);
                    let policies = EnginePolicies {
                        context: policy::Context(&self.policies.context),
                        partition: policy::Partition(&self.policies.partition),
                        state: policy::Selective {
                            inner: &self.policies.state,
                            regions: &coverage.detail_regions,
                        },
                    };
                    // The facts are the original request's identity, not a
                    // newly constructed offline snapshot with the same label.
                    let mut native =
                        AnalysisEngine::new(self.algebra, &policies, &mut *self.extension)
                            .run_metered(
                                AnalysisInput {
                                    world,
                                    entry,
                                    facts,
                                },
                                config,
                                budget,
                                counters,
                                detail_reads,
                                None,
                            )?;
                    coverage.detail_current = current(&native, budget);
                    native.work = budget.used();
                    coverage.detail = if coverage.detail_current {
                        DetailState::Current
                    } else {
                        DetailState::Incomplete
                    };
                    if coverage.detail_current
                        && coverage.detail_binding == HybridNativeBinding::OriginalOwner
                    {
                        coverage.chosen = HybridInterpretation::Native;
                    }
                    if metadata.publish_allowed() {
                        coverage.local_detail_sites =
                            native.states().iter().filter(|state| local(state)).count();
                        if coverage.chosen != HybridInterpretation::Native
                            && coverage.local_detail_sites > 0
                        {
                            coverage.chosen = HybridInterpretation::ProcessWithDetail;
                        }
                    }
                    Ok(Some(native))
                });
                if let Some(result) = result {
                    detail = result?;
                }
                coverage.work.detail = budget
                    .used()
                    .saturating_sub(start)
                    .saturating_sub(coverage.work.preparation);
            }
        }
        coverage.work.total = budget.used();
        Ok(HybridAnalysis {
            baseline,
            detail,
            coverage,
        })
    }
}

#[cfg(test)]
mod tests;
