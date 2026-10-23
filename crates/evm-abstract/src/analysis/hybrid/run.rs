//! Both phases share their caller's native ledger and allocation counters.

use crate::analysis::{
    AnalysisEngine, AnalysisInput, BoundedJoin, ContextInsensitive, ContextPolicy, EnginePolicies,
    EvmWorldAnalysis, ExecutionConfig, FeedbackAcceleration, KnownProcessSummaries,
    MergeResumeStatus, PartitionPolicy, RuntimeExecution, SemanticPolicies, StatePolicy, Status,
    StructuralPartition,
    control::Control,
    cooperation::{Composition, CooperationError},
    engine::{
        self,
        cooperation::{Borrowed, Cooperation},
    },
    transfer::StorageReadPolicy,
};
use crate::{
    domain::{EvmNumericAlgebra, WordElement},
    resource::{CloneError, WorkBudget},
};

use super::{
    DetailState, HybridAnalysis, HybridConfig, HybridCoverage, HybridInterpretation, HybridWork,
    input, local, policy,
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
    /// No whole-root terminal envelope participates in either phase.
    pub fn run_hybrid(
        &mut self,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
        hybrid: HybridConfig,
        control: &Control,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        control.scope(|| {
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            let mut budget = WorkBudget::new(config.max_work);
            let mut counters = engine::Counters::with_observer(control.observer());
            let report = self.run_hybrid_metered(
                input,
                config,
                hybrid,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            )?;
            control
                .checkpoint()
                .map_err(|_| CooperationError::Cancelled)?;
            Ok(report)
        })
    }

    /// Session/RPC adapters must retain their original declared ledger and
    /// counters across source restarts. Never call public run_hybrid as a rescue
    /// after an earlier run or sticky refusal. No kernel graph is rehydrated.
    pub(in crate::analysis) fn run_hybrid_metered(
        &mut self,
        input: AnalysisInput<D::Element>,
        mut config: ExecutionConfig,
        hybrid: HybridConfig,
        budget: &mut WorkBudget,
        counters: &mut engine::Counters,
        storage_reads: StorageReadPolicy,
    ) -> HybridResult<D::Element, <X as Composition<D::Element>>::Error> {
        crate::resource::clone_payment::checkpoint(budget).map_err(|error| match error {
            CloneError::Cancelled => CooperationError::Cancelled,
            CloneError::Work => CooperationError::Protocol(analysis_kernel::KernelError::Stopped(
                crate::analysis::MachineError::work_failure(),
            )),
        })?;
        config.root_terminal_coverage = false;
        let prior = budget.used();
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
        let mut baseline = engine::run_reusing(
            world,
            entry,
            config.clone(),
            self.algebra.borrowed(),
            &coarse,
            budget,
            counters,
            storage_reads,
            None,
            (facts.snapshot(), facts.revision()),
            Cooperation::new(Borrowed(&mut *self.extension)),
        )?;
        let baseline_current = current(&baseline, budget);
        baseline.work = budget.used();
        let mut coverage = HybridCoverage {
            baseline_current,
            detail_current: false,
            local_detail_sites: 0,
            detail_regions: Vec::new(),
            chosen: HybridInterpretation::Process,
            detail: DetailState::BaselinePending,
            requested_detail_work: hybrid.max_detail_work,
            work: HybridWork {
                prior,
                baseline: budget.used().saturating_sub(prior),
                total: budget.used(),
                ..HybridWork::default()
            },
        };
        let mut detail = None;
        if hybrid.max_detail_work == 0 || hybrid.max_detail_regions == 0 {
            coverage.detail = DetailState::Disabled;
        } else if baseline_current && budget.can_afford(0) {
            if self.policies.state.runtime_execution() == RuntimeExecution::Process {
                coverage.detail = DetailState::ProcessPolicy;
            } else {
                let allowance = hybrid
                    .max_detail_work
                    .min(config.max_work.saturating_sub(budget.used()));
                coverage.work.detail_allowance = allowance;
                let start = budget.used();
                let result = budget.with_allowance(allowance, |budget| {
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
                    let (world, entry) = match source {
                        Ok(source) => source,
                        Err(CloneError::Work) => {
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                        Err(CloneError::Cancelled) => return Err(CooperationError::Cancelled),
                    };
                    coverage.detail_regions = match policy::select(
                        &world,
                        entry.address,
                        hybrid.max_detail_regions,
                        budget,
                    ) {
                        Ok(regions) => regions,
                        Err(CloneError::Work) => {
                            coverage.work.preparation = budget.used().saturating_sub(start);
                            coverage.detail = DetailState::Work;
                            return Ok(None);
                        }
                        Err(CloneError::Cancelled) => return Err(CooperationError::Cancelled),
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
                                storage_reads,
                                None,
                            )?;
                    coverage.detail_current = current(&native, budget);
                    native.work = budget.used();
                    coverage.detail = if coverage.detail_current {
                        DetailState::Current
                    } else {
                        DetailState::Incomplete
                    };
                    if coverage.detail_current {
                        coverage.chosen = HybridInterpretation::Native;
                    }
                    if metadata.publish_allowed() {
                        coverage.local_detail_sites =
                            native.states().iter().filter(|state| local(state)).count();
                        if !coverage.detail_current && coverage.local_detail_sites > 0 {
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
