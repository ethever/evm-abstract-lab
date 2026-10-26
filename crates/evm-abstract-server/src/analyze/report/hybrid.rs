//! Preserve both graphs once and export receipts before either graph is moved.
pub(in crate::analyze) mod rpc;
#[cfg(test)]
mod tests;
use super::{ProjectionInput, api, mapping, value};
use crate::analyze::{checkpoint, ssa};
use evm_abstract::{
    analysis::{self, control::Control, layers::SnapshotId},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
    world::SnapshotIdentity,
};
use std::sync::Arc;

struct RetainedPublication {
    minimum: Option<Arc<api::MinimumModel>>,
    programs: Vec<Arc<api::ProgramInfo>>,
    process_controls: Vec<Arc<api::ProcessControlRelation>>,
}

/// Fund the final qualification and fallback directory before optional rich
/// work. Refusal can then retain the exact minimum without a replacement ledger.
fn retained_publication<N: WordElement>(
    source: &analysis::EvmWorldAnalysis<N>,
    input: ProjectionInput<'_>,
    control: &Control,
    retained: Option<&api::AnalysisReport>,
    budget: &mut WorkBudget,
    metadata_work: usize,
) -> Result<RetainedPublication, api::ApiError> {
    let Some(retained) = retained.filter(|report| report.minimum.is_some()) else {
        return Ok(RetainedPublication {
            minimum: None,
            programs: Vec::new(),
            process_controls: Vec::new(),
        });
    };
    let minimum = retained.minimum.as_ref().expect("minimum seed selected");
    checkpoint(control)?;
    if retained.schema_version != api::SCHEMA_VERSION
        || minimum.source_snapshot != snapshot(source.snapshot())?
        || minimum.revision != api::DecimalU64(source.revision().value())
        || minimum.root_binding != mapping::root_binding(source.root_binding())
        || minimum.config != mapping::execution_config(source.config())
        || minimum.fork != input.request.fork
        || minimum.scope != input.scope
        || minimum.numeric_model != input.numeric_model
        || minimum.environment.is_static != source.entry().environment.is_static
        || minimum.environment.input_scope.map(|scope| scope.0)
            != source.entry().environment.input_scope_id()
        || minimum.postprocessing_max_work != budget.limit() as u64
    {
        return Err(invalid(api::HybridValidationKind::Source));
    }
    let required = metadata_work
        .saturating_add(retained.programs.len())
        .saturating_add(retained.process_controls.len())
        .saturating_add(1);
    if (budget.used() as u64) < retained.postprocessing_work || !budget.charge(required) {
        return Err(api::ApiError {
            code: api::ApiErrorCode::Export,
            message: "final publication metadata requires the original postprocessing allowance"
                .into(),
            details: api::ErrorDetails::Export(Box::new(api::ExportFailure {
                kind: api::ExportFailureKind::Work,
                required: required as u64,
                limits: minimum.export_limits,
                usage: minimum.export_usage,
                written: None,
            })),
        });
    }
    Ok(RetainedPublication {
        minimum: Some(Arc::clone(minimum)),
        programs: retained.programs.iter().map(Arc::clone).collect(),
        process_controls: retained.process_controls.iter().map(Arc::clone).collect(),
    })
}

fn finish_publication(
    mut report: api::AnalysisReport,
    rich: Result<(), api::ApiError>,
    budget: &WorkBudget,
) -> Result<api::AnalysisReport, api::ApiError> {
    report.postprocessing_work = budget.used() as u64;
    match rich {
        Ok(()) => report.rich_export = api::RichExportOutcome::Complete,
        Err(error) if report.minimum.is_some() => {
            report.selected_view = api::AnalysisNamespace::Baseline;
            report.rich_export = if error.code == api::ApiErrorCode::Cancelled {
                api::RichExportOutcome::Cancelled
            } else {
                api::RichExportOutcome::Refused(error)
            };
        }
        Err(error) => return Err(error),
    }
    Ok(report)
}

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
    project_with_retained_global(
        source,
        algebra,
        control,
        budget,
        ProjectionInput {
            request,
            scope,
            numeric_model,
        },
        None,
    )
}

/// `retained` is the successful early publication of the exact same producer.
/// The caller must compare ModelHandle identity before supplying it.
pub(in crate::analyze) fn project_with_retained_global<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    source: &analysis::HybridAnalysis<N>,
    algebra: &ValueAlgebra<D>,
    control: &Control,
    budget: &mut WorkBudget,
    input: ProjectionInput<'_>,
    retained: Option<&api::AnalysisReport>,
) -> Result<api::AnalysisReport, api::ApiError> {
    let metadata_work = source.coverage().detail_regions.len().saturating_add(1);
    let RetainedPublication {
        minimum,
        programs,
        process_controls,
    } = retained_publication(
        source.baseline(),
        input,
        control,
        retained,
        budget,
        metadata_work,
    )?;
    let mut report = api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        selected_view: api::AnalysisNamespace::Baseline,
        minimum,
        rich_export: api::RichExportOutcome::NotRequested,
        status: match source.status() {
            analysis::Status::Converged => api::AnalysisStatus::Converged,
            analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
        },
        work: source.work() as u64,
        transfers: source.transfers() as u64,
        postprocessing_work: budget.used() as u64,
        timings: api::AnalysisTimings::default(),
        acquisition: source
            .baseline()
            .rpc_acquisition()
            .map(crate::analyze::rpc::acquisition),
        programs,
        process_controls,
        global: None,
        native: None,
        hybrid: Some(api::HybridReport {
            coverage: coverage(source)?,
            local_prefixes: Vec::new(),
            precision: None,
            rpc: None,
        }),
    };
    let rich = (|| {
        // Commit rich references only after both namespace projections succeed.
        // A failed Native export cannot leave receipts pointing into absent pools.
        let local_prefixes = prefixes(source, control, budget)?;
        let super::GraphProjection {
            graph: global,
            mut programs,
            mut process_controls,
        } = super::project_global(source.baseline(), algebra, control, budget, input, retained)?;
        let native = source
            .detail()
            .map(|detail| {
                let super::GraphProjection {
                    graph,
                    programs: catalog,
                    process_controls: controls,
                } = super::project_graph(
                    detail,
                    algebra,
                    control,
                    budget,
                    input,
                    std::mem::take(&mut programs),
                    std::mem::take(&mut process_controls),
                )?;
                programs = catalog;
                process_controls = controls;
                Ok::<_, api::ApiError>(graph)
            })
            .transpose()?;
        let selected_view = match source.coverage().chosen {
            analysis::HybridInterpretation::Native => api::AnalysisNamespace::Native,
            analysis::HybridInterpretation::Process
            | analysis::HybridInterpretation::ProcessWithDetail => api::AnalysisNamespace::Baseline,
        };
        if selected_view == api::AnalysisNamespace::Native && native.is_none() {
            return Err(invalid(api::HybridValidationKind::Namespace));
        }
        checkpoint(control)?;
        report.selected_view = selected_view;
        report.global = Some(global);
        report.native = native;
        report.programs = programs;
        report.process_controls = process_controls;
        report
            .hybrid
            .as_mut()
            .expect("hybrid envelope constructed")
            .local_prefixes = local_prefixes;
        Ok(())
    })();
    finish_publication(report, rich, budget)
}

fn invalid(kind: api::HybridValidationKind) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: format!("invalid native Hybrid projection: {kind:?}"),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Hybrid(kind)),
    }
}

pub(super) fn snapshot(source: &SnapshotId) -> Result<api::RootTerminalSnapshot, api::ApiError> {
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
        requested_detail_regions: coverage.requested_detail_regions as u64,
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
