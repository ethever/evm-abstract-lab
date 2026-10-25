//! Preserve both graphs once and export receipts before either graph is moved.
pub(in crate::analyze) mod rpc;
use super::{api, mapping, value};
use crate::analyze::{checkpoint, ssa};
use evm_abstract::{
    analysis::{self, control::Control, layers::SnapshotId},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
    world::SnapshotIdentity,
};

pub(in crate::analyze) fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &analysis::HybridAnalysis<N>,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    project_with_budget(
        source,
        request,
        scope,
        algebra,
        numeric_model,
        control,
        &mut budget,
    )
}

pub(in crate::analyze) fn project_with_budget<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &analysis::HybridAnalysis<N>,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
    budget: &mut WorkBudget,
) -> Result<api::AnalysisReport, api::ApiError> {
    // EvmWorldAnalysis serde omits execution_evidence. Export the borrowed
    // receipts explicitly, with the native entry IDs and frontier namespace.
    let local_prefixes = prefixes(source, control, budget)?;
    let coverage = coverage(source)?;
    let (baseline, mut programs) = super::project_graph(
        source.baseline(),
        request,
        scope,
        algebra,
        numeric_model,
        control,
        budget,
        Vec::new(),
    )?;
    let detail = source
        .detail()
        .map(|detail| {
            let (graph, catalog) = super::project_graph(
                detail,
                request,
                scope,
                algebra,
                numeric_model,
                control,
                budget,
                std::mem::take(&mut programs),
            )?;
            programs = catalog;
            Ok::<_, api::ApiError>(graph)
        })
        .transpose()?;
    let selected_view = match coverage.chosen {
        api::HybridInterpretation::Native => api::AnalysisNamespace::Native,
        api::HybridInterpretation::Process | api::HybridInterpretation::ProcessWithDetail => {
            api::AnalysisNamespace::Baseline
        }
    };
    if selected_view == api::AnalysisNamespace::Native && detail.is_none() {
        return Err(invalid(api::HybridValidationKind::Namespace));
    }
    let report = api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        selected_view,
        status: match source.status() {
            analysis::Status::Converged => api::AnalysisStatus::Converged,
            analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
        },
        work: source.work() as u64,
        transfers: source
            .baseline()
            .transfers()
            .saturating_add(source.detail().map_or(0, |detail| detail.transfers()))
            as u64,
        postprocessing_work: budget.used() as u64,
        timings: api::AnalysisTimings::default(),
        programs,
        global: Some(baseline),
        native: detail,
        hybrid: Some(api::HybridReport {
            coverage,
            local_prefixes,
            precision: None,
            rpc: None,
        }),
    };
    checkpoint(control)?;
    Ok(report)
}

fn invalid(kind: api::HybridValidationKind) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: format!("invalid native Hybrid projection: {kind:?}"),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Hybrid(kind)),
    }
}

fn snapshot(source: &SnapshotId) -> Result<api::RootTerminalSnapshot, api::ApiError> {
    Ok(match source.identity() {
        SnapshotIdentity::Offline { label } => {
            api::RootTerminalSnapshot::Offline(api::RootTerminalOfflineSnapshot {
                label: label.clone(),
                namespace: api::DecimalU64(
                    source
                        .offline_namespace()
                        .ok_or_else(|| invalid(api::HybridValidationKind::Source))?,
                ),
            })
        }
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => api::RootTerminalSnapshot::Chain(api::RootTerminalChainSnapshot {
            chain_id: value::word(*chain_id),
            block_hash: block_hash.to_string(),
        }),
    })
}

pub(in crate::analyze) fn coverage<N: WordElement>(
    source: &analysis::HybridAnalysis<N>,
) -> Result<api::HybridCoverage, api::ApiError> {
    let coverage = source.coverage();
    let work = coverage.work;
    Ok(api::HybridCoverage {
        snapshot: snapshot(source.baseline().snapshot())?,
        baseline_revision: api::DecimalU64(source.baseline().revision().value()),
        detail_revision: source
            .detail()
            .map(|detail| api::DecimalU64(detail.revision().value())),
        baseline_config: mapping::execution_config(source.baseline().config()),
        detail_config: source
            .detail()
            .map(|detail| mapping::execution_config(detail.config())),
        baseline_current: coverage.baseline_current,
        detail_current: coverage.detail_current,
        detail_binding: binding(coverage.detail_binding),
        requested_native_terminal_coverage: coverage.requested_native_terminal_coverage,
        local_detail_sites: coverage.local_detail_sites,
        detail_regions: coverage
            .detail_regions
            .iter()
            .map(|region| api::HybridRegion {
                source: crate::analyze::process::source(region.source),
                code_address: match region.source {
                    analysis::ProcessSource::Runtime { source, .. } => Some(source.to_string()),
                    _ => None,
                },
                code_hash: region.code_hash.to_string(),
            })
            .collect(),
        chosen: match coverage.chosen {
            analysis::HybridInterpretation::Native => api::HybridInterpretation::Native,
            analysis::HybridInterpretation::Process => api::HybridInterpretation::Process,
            analysis::HybridInterpretation::ProcessWithDetail => {
                api::HybridInterpretation::ProcessWithDetail
            }
        },
        detail: match coverage.detail {
            analysis::DetailState::Pending => api::HybridDetailState::Pending,
            analysis::DetailState::Disabled => api::HybridDetailState::Disabled,
            analysis::DetailState::BaselinePending => api::HybridDetailState::BaselinePending,
            analysis::DetailState::ProcessPolicy => api::HybridDetailState::ProcessPolicy,
            analysis::DetailState::Work => api::HybridDetailState::Work,
            analysis::DetailState::Incomplete => api::HybridDetailState::Incomplete,
            analysis::DetailState::Current => api::HybridDetailState::Current,
        },
        requested_detail_work: api::DecimalU64(coverage.requested_detail_work as u64),
        work: api::HybridWork {
            prior: work.prior as u64,
            baseline: work.baseline as u64,
            preparation: work.preparation as u64,
            detail: work.detail as u64,
            detail_allowance: work.detail_allowance as u64,
            total: work.total as u64,
        },
    })
}

fn prefixes<N: WordElement>(
    source: &analysis::HybridAnalysis<N>,
    control: &Control,
    budget: &mut WorkBudget,
) -> Result<Vec<api::HybridLocalPrefix>, api::ApiError> {
    let mut prefixes = Vec::new();
    for prefix in source.local_detail() {
        checkpoint(control)?;
        if !budget.charge(
            32usize
                .saturating_add(prefix.frame.jump_history.len())
                .saturating_add(prefix.executed_pcs.len())
                .saturating_add(prefix.successors.len())
                .saturating_add(source.detail().map_or(0, |detail| detail.frontiers().len())),
        ) {
            return Err(ssa::errors::convert(evm_abstract::ssa::SsaError::Work));
        }
        let frame = prefix.frame;
        if frame.process.is_some() || prefix.executed_pcs.len() != prefix.instructions.len() {
            return Err(invalid(api::HybridValidationKind::Receipt));
        }
        prefixes.push(api::HybridLocalPrefix {
            namespace: api::AnalysisNamespace::Native,
            state: prefix.state_id,
            scope: match prefix.scope {
                analysis::HybridLocalScope::RetainedInputPrefix => {
                    api::HybridLocalScope::RetainedInputPrefix
                }
                analysis::HybridLocalScope::NativeOwnerNamespacePrefix => {
                    api::HybridLocalScope::NativeOwnerNamespacePrefix
                }
            },
            binding: binding(prefix.binding),
            snapshot: snapshot(prefix.snapshot)?,
            revision: api::DecimalU64(prefix.revision.value()),
            frame: mapping::regional_frame(frame, true),
            context_selected: prefix.context_selected,
            instructions: prefix
                .executed_pcs
                .iter()
                .zip(prefix.instructions)
                .map(|(pc, phase)| api::HybridInstruction {
                    pc: *pc,
                    phase: mapping::instruction_progress(*phase),
                })
                .collect(),
            successors: prefix
                .successors
                .iter()
                .map(|(state, kind)| api::HybridSuccessor {
                    state: *state,
                    kind: mapping::edge(*kind),
                })
                .collect(),
            frontiers: source
                .detail()
                .into_iter()
                .flat_map(|detail| detail.frontiers().iter().enumerate())
                .filter(|(_, frontier)| frontier.from == Some(prefix.state_id))
                .map(|(index, _)| index)
                .collect(),
        });
    }
    Ok(prefixes)
}

fn binding(source: analysis::HybridNativeBinding) -> api::HybridNativeBinding {
    match source {
        analysis::HybridNativeBinding::OriginalOwner => api::HybridNativeBinding::OriginalOwner,
        analysis::HybridNativeBinding::SymbolicOwnerNamespace { namespace } => {
            api::HybridNativeBinding::SymbolicOwnerNamespace(api::HybridOwnerNamespace {
                namespace: namespace.to_string(),
            })
        }
    }
}
