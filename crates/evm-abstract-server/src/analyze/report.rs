//! Project the complete world machine while preserving native graph namespaces.
//! The caller retains the actual execution algebra; the model label comes from
//! application dispatch, while captured numeric limits come from native results.
mod environment;
pub(super) mod hybrid;
mod summaries;
mod terminal;
use super::{checkpoint, mapping, pools::Pools, ssa, value};
use evm_abstract::{
    analysis::{self, control::Control, progress::Phase},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
    world::{EvmStore, SnapshotIdentity},
};
use evm_abstract_protocol as api;
use std::sync::Arc;

/// Frozen application inputs shared by both namespace projections.
#[derive(Clone, Copy)]
pub(in crate::analyze) struct ProjectionInput<'a> {
    pub request: &'a api::AnalyzeRequest,
    pub scope: api::AnalysisScope,
    pub numeric_model: api::NumericModel,
}

/// Reuse is admitted by the publisher's exact ModelHandle identity. These
/// structural guards additionally reject a seed from another input or policy.
/// Cloning catalog handles preserves all assigned IDs without copying instructions.
fn project_global<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    native: &analysis::EvmWorldAnalysis<N>,
    algebra: &ValueAlgebra<D>,
    control: &Control,
    budget: &mut WorkBudget,
    input: ProjectionInput<'_>,
    retained: Option<&api::AnalysisReport>,
) -> Result<(Arc<api::AnalysisGraph>, Vec<Arc<api::ProgramInfo>>), api::ApiError> {
    checkpoint(control)?;
    let Some(retained) = retained else {
        let (graph, programs) = project_graph(native, algebra, control, budget, input, Vec::new())?;
        return Ok((Arc::new(graph), programs));
    };
    let invalid = || api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: "retained Global projection has a different input".into(),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source,
        )),
    };
    let graph = retained.global.as_ref().ok_or_else(invalid)?;
    if retained.schema_version != api::SCHEMA_VERSION
        || graph.metadata.source_snapshot != hybrid::snapshot(native.snapshot())?
        || graph.metadata.revision != api::DecimalU64(native.revision().value())
        || graph.metadata.root_binding != mapping::root_binding(native.root_binding())
        || graph.metadata.limits != input.request.limits
        || graph.fork != input.request.fork
        || graph.scope != input.scope
    {
        return Err(invalid());
    }
    if !budget.charge(retained.programs.len().saturating_add(1)) {
        return Err(ssa::errors::convert(evm_abstract::ssa::SsaError::Work));
    }
    let programs = retained.programs.iter().map(Arc::clone).collect();
    checkpoint(control)?;
    Ok((Arc::clone(graph), programs))
}

#[cfg(any(test, feature = "capture-replay"))]
pub(super) fn report<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    native: &analysis::EvmWorldAnalysis<N>,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    let mut postprocessing = WorkBudget::new(super::DEFAULT_POSTPROCESS_WORK);
    with_budget(
        native,
        request,
        scope,
        algebra,
        numeric_model,
        control,
        &mut postprocessing,
    )
}

#[cfg(any(test, feature = "capture-replay"))]
pub(in crate::analyze) fn with_budget<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    native: &analysis::EvmWorldAnalysis<N>,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
    postprocessing: &mut WorkBudget,
) -> Result<api::AnalysisReport, api::ApiError> {
    let (graph, programs) = project_graph(
        native,
        algebra,
        control,
        postprocessing,
        ProjectionInput {
            request,
            scope,
            numeric_model,
        },
        Vec::new(),
    )?;
    Ok(api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        selected_view: api::AnalysisNamespace::Native,
        status: graph.status,
        work: graph.metadata.work,
        transfers: graph.transfers,
        postprocessing_work: postprocessing.used() as u64,
        timings: api::AnalysisTimings::default(),
        acquisition: native.rpc_acquisition().map(super::rpc::acquisition),
        programs,
        global: None,
        native: Some(Arc::new(graph)),
        hybrid: None,
    })
}

/// Thread one immutable source directory through both namespace projections.
/// Graph-local numerical pools are deliberately independent.
fn project_graph<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    native: &analysis::EvmWorldAnalysis<N>,
    algebra: &ValueAlgebra<D>,
    control: &Control,
    postprocessing: &mut WorkBudget,
    input: ProjectionInput<'_>,
    programs: Vec<std::sync::Arc<api::ProgramInfo>>,
) -> Result<(api::AnalysisGraph, Vec<std::sync::Arc<api::ProgramInfo>>), api::ApiError> {
    let ProjectionInput {
        request,
        scope,
        numeric_model,
    } = input;
    let initial_work = postprocessing.used();
    checkpoint(control)?;
    // The request's Control.scope covers this ledger and every native output
    // computation. There is no local execution projection on the server path.
    let mut pools = Pools::<N>::with_programs(control, programs);
    let root_binding = mapping::root_binding(native.root_binding());
    pools.root_namespace = match native.root_binding() {
        evm_abstract::world::RootInputBinding::Namespace { coordinate } => Some(coordinate),
        evm_abstract::world::RootInputBinding::Physical { .. } => None,
    };
    let declared_root_source = native
        .declared_root_source(postprocessing)
        .map(super::process::source)
        .map_err(|error| match error {
            analysis::ProjectionError::Cancelled => super::cancelled(),
            analysis::ProjectionError::Work => {
                ssa::errors::convert(evm_abstract::ssa::SsaError::Work)
            }
            analysis::ProjectionError::MissingProgram
            | analysis::ProjectionError::NonRuntime(_) => {
                super::process::invalid(api::ProcessValidationKind::Source)
            }
        })?;
    pools.symbolic.charge(
        native
            .states()
            .len()
            .saturating_add(native.edges().len())
            .saturating_add(native.diagnostics().len())
            .saturating_add(native.frontiers().len())
            .saturating_add(native.outcomes().len())
            .saturating_add(native.world().accounts().len()),
    )?;
    control.observer().phase(Phase::BuildingSsa);
    let ssa = ssa::report(native, control, postprocessing, &mut pools.symbolic)?;
    checkpoint(control)?;
    control.observer().phase(Phase::Projecting);
    let initial =
        EvmStore::<N>::new_checked(native.world(), algebra.borrowed()).map_err(|error| {
            if control.cancellation().is_cancelled() {
                return pools.symbolic.failure(api::ExportFailureKind::Cancelled, 0);
            }
            let mut failure = pools.symbolic.failure(api::ExportFailureKind::StateRoot, 0);
            failure.message = format!("initial store export failed: {error}");
            failure
        })?;
    let accounts: Vec<api::AccountState> = native
        .world()
        .accounts()
        .keys()
        .map(|address| pools.account(&initial, *address))
        .collect::<Result<_, _>>()?;
    let mut states = Vec::with_capacity(native.states().len());
    let mut cfg = Vec::with_capacity(native.states().len());
    for state in native.states() {
        checkpoint(control)?;
        let entry = pools.machine(&state.entry)?;
        let program = entry.frames.last().and_then(|frame| frame.program);
        let process_key = state.active().process.as_deref().map(super::process::key);
        let is_native = process_key.is_none();
        let block = is_native
            .then(|| state.program())
            .flatten()
            .and_then(|program| program.blocks().get(state.active().basic_block_index));
        pools.symbolic.charge(
            state
                .active()
                .jump_history
                .len()
                .saturating_add(state.executed_pcs.len())
                .saturating_add(state.exit_stack.len())
                .saturating_add(1),
        )?;
        cfg.push(api::CfgBlock {
            process: process_key,
            id: state.id,
            basic_block: is_native.then_some(state.active().basic_block_index),
            start_pc: block.map(|block| block.start_pc),
            context: if is_native {
                state.active().jump_history.clone()
            } else {
                vec![]
            },
            frame_depth: state.key.frames.len(),
            code_address: is_native.then(|| state.active().code_address.to_string()),
            storage_address: is_native.then(|| state.active().address.to_string()),
            program,
            entry_stack: if is_native {
                state
                    .entry
                    .active()
                    .stack
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            } else {
                vec![]
            },
            exit_stack: if is_native {
                state.exit_stack.iter().map(ToString::to_string).collect()
            } else {
                vec![]
            },
            executed_pcs: if is_native {
                state.executed_pcs.clone()
            } else {
                vec![]
            },
        });
        states.push(api::StateDetails {
            state: state.id,
            entry,
            exit: state
                .exit
                .as_ref()
                .map(|payload| pools.machine(payload))
                .transpose()?,
        });
    }
    let mut outcomes = Vec::with_capacity(native.outcomes().len());
    for outcome in native.outcomes() {
        checkpoint(control)?;
        outcomes.push(api::Outcome {
            state: outcome.state,
            kind: match outcome.kind {
                analysis::OutcomeKind::Return => api::OutcomeKind::Return,
                analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
                analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
            },
            data: pools.buffer(&outcome.data)?,
            store: pools.store(&outcome.store)?,
        });
    }
    let root_program = states
        .first()
        .and_then(|state| state.entry.frames.first())
        .and_then(|frame| frame.program)
        .or_else(|| {
            // This is paid declared provenance, including zero-state refusals.
            // A namespace marker/coordinate never becomes a physical lookup.
            pools
                .programs
                .iter()
                .find(|program| match &declared_root_source {
                    api::ProcessSource::InputProgram(source) => {
                        program.code_address.is_none()
                            && program.kind == api::CodeKind::Runtime
                            && program.code_hash == source.hash.to_string()
                    }
                    api::ProcessSource::Runtime(source) => {
                        program.code_address.as_ref() == Some(&source.source)
                            && program.kind == api::CodeKind::Runtime
                            && program.code_hash == source.hash.to_string()
                    }
                    _ => false,
                })
                .map(|program| program.id)
        });
    let byte_len = root_program.map_or(0, |id| {
        pools.programs[id].bytecode.len().saturating_sub(2) / 2
    });
    let world = native.world();
    let snapshot = match world.identity() {
        SnapshotIdentity::Offline { .. } => None,
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => {
            let facts = world.snapshot_environment();
            Some(api::ChainSnapshot {
                chain_id: value::word(*chain_id),
                block_hash: block_hash.to_string(),
                number: facts.map(|facts| value::word(facts.number)),
                parent_hash: facts.map(|facts| facts.parent_hash.to_string()),
                timestamp: facts.map(|facts| value::word(facts.timestamp)),
                coinbase: facts.map(|facts| facts.coinbase.to_string()),
                prevrandao: facts.map(|facts| facts.prevrandao.to_string()),
                gas_limit: facts.map(|facts| value::word(facts.gas_limit)),
                base_fee: facts.and_then(|facts| facts.base_fee).map(value::word),
                blob_base_fee: facts.and_then(|facts| facts.blob_base_fee).map(value::word),
                excess_blob_gas: facts
                    .and_then(|facts| facts.excess_blob_gas)
                    .map(value::word),
                blob_gas_used: facts.and_then(|facts| facts.blob_gas_used).map(value::word),
            })
        }
    };
    let environment = environment::project(&native.entry().environment, &mut pools)?;
    checkpoint(control)?;
    let abstract_summaries =
        summaries::project(native.procedure_summaries(), control, &mut pools.symbolic)?;
    let root_terminal_coverage = native
        .root_terminal_coverage()
        .map(|source| terminal::project(source, numeric_model, &mut pools))
        .transpose()?;
    checkpoint(control)?;
    let report = api::AnalysisGraph {
        scope,
        fork: request.fork,
        byte_len,
        status: match native.status() {
            analysis::Status::Converged => api::AnalysisStatus::Converged,
            analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
        },
        transfers: native.transfers() as u64,
        cfg,
        edges: native
            .edges()
            .iter()
            .enumerate()
            .map(|(id, edge)| api::CfgEdge {
                id,
                from: edge.from,
                to: edge.to,
                kind: mapping::edge(edge.kind),
            })
            .collect(),
        ssa,
        diagnostics: native
            .diagnostics()
            .iter()
            .map(|item| api::Diagnostic {
                state: item.state,
                pc: item.pc,
                kind: mapping::diagnostic(&item.kind),
                reduction: mapping::reduction(&item.kind),
                precision: mapping::precision(&item.kind),
                invalid_opcode: mapping::invalid_opcode(&item.kind),
                detail: match &item.kind {
                    analysis::DiagnosticKind::InvalidOpcode(reason) => reason.to_string(),
                    other => format!("{other:?}"),
                },
            })
            .collect(),
        frontiers: native
            .frontiers()
            .iter()
            .map(|item| {
                Ok(api::Frontier {
                    from: item.from,
                    pc: item.pc,
                    kind: mapping::frontier(&item.reason),
                    reason: mapping::reason(&item.reason),
                    detail: format!("{:?}", item.reason),
                    range: item
                        .range
                        .as_ref()
                        .map(|witness| pools.range(witness))
                        .transpose()?,
                    payload: item
                        .payload
                        .as_ref()
                        .map(|payload| pools.pending(payload))
                        .transpose()?,
                })
            })
            .collect::<Result<_, api::ApiError>>()?,
        metadata: api::ReportMetadata {
            entry_address: native.entry().address.to_string(),
            root_binding,
            declared_root_source,
            root_program,
            source_snapshot: hybrid::snapshot(native.snapshot())?,
            revision: api::DecimalU64(native.revision().value()),
            snapshot,
            fingerprint: world
                .fingerprint()
                .map_err(super::errors::fingerprint)?
                .to_string(),
            environment,
            limits: request.limits.clone(),
            work: native.work() as u64,
            postprocessing_max_work: super::DEFAULT_POSTPROCESS_WORK as u64,
            postprocessing_work: postprocessing.used().saturating_sub(initial_work) as u64,
            export_limits: pools.symbolic.limits,
            export_usage: pools.symbolic.usage,
        },
        symbolic: pools.symbolic.graph,
        pending_machines: pools.pending,
        accounts,
        states,
        byte_arrays: pools.buffers_dto,
        stores: pools.stores,
        outcomes,
        root_terminal_coverage,
        abstract_summaries,
    };
    checkpoint(control)?;
    Ok((report, pools.programs))
}
