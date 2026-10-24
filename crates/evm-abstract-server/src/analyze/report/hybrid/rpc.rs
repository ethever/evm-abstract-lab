//! RPC preserves its original upper bound and the last useful native revision.
use super::{api, coverage, prefixes, snapshot};
use crate::analyze::{checkpoint, errors, rpc as evidence};
use evm_abstract::{
    analysis::{self, control::Control},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
};

pub(in crate::analyze) fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: analysis::RpcHybridAnalysis<N>,
    request: &api::AnalyzeRequest,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    // Project borrowed execution evidence before into_parts moves the graphs.
    let local_prefixes = source
        .detail_report()
        .map(|detail| prefixes(detail, control, &mut budget))
        .transpose()?
        .unwrap_or_default();
    let original = coverage(source.baseline())?;
    let precision = source.detail_report().map(coverage).transpose()?;
    let status = source.status();
    let (baseline, latest, rpc, failures, preparation_failure, retained_detail) =
        source.into_parts();
    let rpc = api::RpcHybridCoverage {
        snapshot: snapshot(&rpc.snapshot)?,
        baseline_revision: api::DecimalU64(rpc.baseline_revision.value()),
        current_revision: api::DecimalU64(rpc.current_revision.value()),
        detail_revision: rpc
            .detail_revision
            .map(|revision| api::DecimalU64(revision.value())),
        detail_at_current_revision: rpc.detail_at_current_revision,
        baseline_relation: match rpc.baseline_relation {
            analysis::RpcBaselineRelation::Uncertified => api::RpcBaselineRelation::Uncertified,
            analysis::RpcBaselineRelation::SameRevision => api::RpcBaselineRelation::SameRevision,
            analysis::RpcBaselineRelation::InitialFactRestriction => {
                api::RpcBaselineRelation::InitialFactRestriction
            }
        },
        restrictions: rpc
            .restrictions
            .iter()
            .map(|restriction| api::RpcFactRestriction {
                previous: api::DecimalU64(restriction.previous.value()),
                current: api::DecimalU64(restriction.current.value()),
                added_observations: restriction.added_observations as u64,
            })
            .collect(),
        precision: match rpc.precision {
            analysis::RpcPrecisionOutcome::Disabled => api::RpcPrecisionOutcome::Disabled,
            analysis::RpcPrecisionOutcome::BaselinePending => {
                api::RpcPrecisionOutcome::BaselinePending
            }
            analysis::RpcPrecisionOutcome::Current => api::RpcPrecisionOutcome::Current,
            analysis::RpcPrecisionOutcome::Incomplete => api::RpcPrecisionOutcome::Incomplete,
            analysis::RpcPrecisionOutcome::Work => api::RpcPrecisionOutcome::Work,
            analysis::RpcPrecisionOutcome::Budget(limit) => {
                api::RpcPrecisionOutcome::Budget(match limit {
                    analysis::Limit::States => api::HybridBudget::States,
                    analysis::Limit::Transfers => api::HybridBudget::Transfers,
                    analysis::Limit::Work => api::HybridBudget::Work,
                    analysis::Limit::CallDepth => api::HybridBudget::CallDepth,
                    analysis::Limit::Memory => api::HybridBudget::Memory,
                    analysis::Limit::Model => api::HybridBudget::Model,
                })
            }
            analysis::RpcPrecisionOutcome::AcquisitionFailed => {
                api::RpcPrecisionOutcome::AcquisitionFailed
            }
            analysis::RpcPrecisionOutcome::PreparationFailed => {
                api::RpcPrecisionOutcome::PreparationFailed
            }
        },
        acquisition: evidence::acquisition(&rpc.acquisition),
        work: rpc.work as u64,
        precision_work: rpc.precision_work as u64,
        max_precision_work: api::DecimalU64(rpc.max_precision_work as u64),
        transfers: rpc.transfers as u64,
        failures: failures
            .iter()
            .map(|error| evidence::failure(&error.failure()))
            .collect(),
        preparation_failure: preparation_failure.map(errors::session_error).map(Box::new),
    };
    let (baseline, _, _) = baseline.into_parts();
    let native = latest
        .and_then(|report| report.into_parts().1)
        .or_else(|| retained_detail.and_then(|report| report.into_parts().1));
    let mut report = super::super::with_budget(
        &baseline,
        request,
        api::AnalysisScope::RpcWorld,
        algebra,
        numeric_model,
        control,
        &mut budget,
    )?;
    let detail = native
        .as_ref()
        .map(|native| {
            super::super::with_budget(
                native,
                request,
                api::AnalysisScope::RpcWorld,
                algebra,
                numeric_model,
                control,
                &mut budget,
            )
        })
        .transpose()?;
    report.status = match status {
        analysis::Status::Converged => api::AnalysisStatus::Converged,
        analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
    };
    report.metadata.work = rpc.work;
    report.metadata.postprocessing_work = budget.used() as u64;
    report.transfers = rpc.transfers;
    report.acquisition = Some(rpc.acquisition.clone());
    report.hybrid = Some(Box::new(api::HybridReport {
        coverage: original,
        baseline: None,
        detail: detail.map(Box::new),
        local_prefixes,
        precision,
        rpc: Some(rpc),
    }));
    checkpoint(control)?;
    Ok(report)
}
