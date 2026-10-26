//! RPC preserves its original upper bound and the last useful native revision.
use super::{
    ProjectionInput, RetainedPublication, api, coverage, finish_publication, prefixes,
    retained_publication, snapshot,
};
use crate::analyze::{checkpoint, errors, rpc as evidence};
use evm_abstract::{
    analysis::{self, control::Control},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
};

/// Reuse only a publication admitted by exact immutable producer identity.
pub(in crate::analyze) fn project_with_retained_global<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    source: analysis::RpcHybridAnalysis<N>,
    algebra: &ValueAlgebra<D>,
    control: &Control,
    budget: &mut WorkBudget,
    input: ProjectionInput<'_>,
    retained: Option<&api::AnalysisReport>,
) -> Result<api::AnalysisReport, api::ApiError> {
    // Reserve final RPC qualification before optional pools/SSA. The source is
    // consumed only after its actual revision chain has been retained here.
    let rpc_metadata = source.coverage();
    let acquisition = &rpc_metadata.acquisition;
    let metadata_work = rpc_metadata
        .restrictions
        .len()
        .saturating_add(source.failures().len())
        .saturating_add(acquisition.fetched_accounts.len())
        .saturating_add(acquisition.failed_accounts.len())
        .saturating_add(acquisition.fetched_storage.len())
        .saturating_add(acquisition.failed_storage.len())
        .saturating_add(acquisition.failures.len())
        .saturating_mul(2)
        .saturating_add(1);
    let RetainedPublication { minimum, programs } = retained_publication(
        source.baseline().baseline(),
        input,
        control,
        retained,
        budget,
        metadata_work,
    )?;
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
    let native_report = latest
        .as_ref()
        .filter(|report| report.detail().is_some())
        .or(retained_detail.as_ref());
    let mut report = api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        selected_view: api::AnalysisNamespace::Baseline,
        minimum,
        rich_export: api::RichExportOutcome::NotRequested,
        status: match status {
            analysis::Status::Converged => api::AnalysisStatus::Converged,
            analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
        },
        work: rpc.work,
        transfers: rpc.transfers,
        postprocessing_work: budget.used() as u64,
        timings: api::AnalysisTimings::default(),
        acquisition: Some(rpc.acquisition.clone()),
        programs,
        global: None,
        native: None,
        hybrid: Some(api::HybridReport {
            coverage: original,
            local_prefixes: Vec::new(),
            precision,
            rpc: Some(rpc),
        }),
    };
    let rich = (|| {
        let local_prefixes = native_report
            .map(|detail| prefixes(detail, control, budget))
            .transpose()?
            .unwrap_or_default();
        let (global, mut programs) = super::super::project_global(
            baseline.baseline(),
            algebra,
            control,
            budget,
            input,
            retained,
        )?;
        let native = native_report
            .and_then(|report| report.detail())
            .map(|native| {
                let (graph, catalog) = super::super::project_graph(
                    native,
                    algebra,
                    control,
                    budget,
                    input,
                    std::mem::take(&mut programs),
                )?;
                programs = catalog;
                Ok::<_, api::ApiError>(std::sync::Arc::new(graph))
            })
            .transpose()?;
        checkpoint(control)?;
        report.global = Some(global);
        report.native = native;
        report.programs = programs;
        report
            .hybrid
            .as_mut()
            .expect("RPC hybrid envelope constructed")
            .local_prefixes = local_prefixes;
        Ok(())
    })();
    finish_publication(report, rich, budget)
}
