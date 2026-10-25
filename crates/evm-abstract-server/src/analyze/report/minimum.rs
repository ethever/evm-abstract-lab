//! Mandatory semantic publication, before optional SSA/private-state export.
//!
//! Closure/currentness come only from the existing Hybrid result. Canonical
//! procedure rows are borrowed directly; this module never replays a summary.
mod numeric;
#[cfg(test)]
mod tests;

use super::{ProjectionInput, api, hybrid, mapping, summaries, value};
use crate::analyze::{checkpoint, pools::Pools, process, symbolic};
use evm_abstract::{
    analysis::{self, control::Control},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement, WordObservation},
    resource::WorkBudget,
    world::{Code, EvmEnvironmentValues, GasInput, SnapshotIdentity},
};
use std::sync::Arc;

/// The same application ledger must fund this mandatory model first. A caller
/// retains the returned immutable model before attempting any rich projection.
pub(in crate::analyze) fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &analysis::HybridAnalysis<N>,
    algebra: &ValueAlgebra<D>,
    control: &Control,
    budget: &mut WorkBudget,
    input: ProjectionInput<'_>,
) -> Result<api::AnalysisReport, api::ApiError> {
    checkpoint(control)?;
    let native = source.baseline();
    // This copies library qualification, never infers closure from an export.
    if !source.coverage().baseline_current || native.status() != analysis::Status::Converged {
        return Err(invalid(api::MinimumValidationKind::Closure));
    }
    let initial_work = budget.used();
    let postprocessing_max_work = budget.limit() as u64;
    let declared_root_source = native.declared_root_source(budget);
    let provenance_work = budget.used().saturating_sub(initial_work);
    let mut pools = Pools::with_programs(control, Vec::new());
    pools.symbolic = symbolic::Pool::minimum(control, budget);
    pools.symbolic.usage.work = provenance_work as u64;
    let declared_root_source = declared_root_source.map_err(|error| match error {
        analysis::ProjectionError::Cancelled => crate::analyze::cancelled(),
        // The native refusal carries no requested amount; zero explicitly
        // records an unavailable enclosing quote, never fictitious SSA work.
        analysis::ProjectionError::Work => pools.symbolic.failure(api::ExportFailureKind::Work, 0),
        _ => invalid(api::MinimumValidationKind::Source),
    })?;
    pools.root_namespace = match native.root_binding() {
        evm_abstract::world::RootInputBinding::Namespace { coordinate } => Some(coordinate),
        evm_abstract::world::RootInputBinding::Physical { .. } => None,
    };
    let label_work = match native.world().identity() {
        SnapshotIdentity::Offline { label } => label.len(),
        SnapshotIdentity::Chain { .. } => 128,
    };
    pools.symbolic.payload(label_work)?;
    pools.symbolic.charge(
        source
            .coverage()
            .detail_regions
            .len()
            .saturating_mul(128)
            .saturating_add(256),
    )?;
    let coverage = hybrid::coverage(source)?;

    let accounts = accounts(native, &mut pools)?;
    let fingerprint = fingerprint(native, &mut pools)?;
    let environment = environment(&native.entry().environment, algebra, &mut pools)?;
    let observations = native.procedure_summaries();
    // Minimum-mode values use the existing scalar/receipt mapper while
    // refusing private expressions and unsupported custom predicates.
    let relations = summaries::project(observations, control, &mut pools.symbolic)?;
    pools
        .symbolic
        .charge(observations.len().saturating_mul(4))?;
    let mut output = Vec::with_capacity(observations.len());
    let mut tables = Vec::with_capacity(observations.len());
    for (observation, relation) in observations.iter().zip(relations) {
        let bounds = bounds(observation, &mut tables, &mut pools)?;
        output.push(api::MinimumSummary {
            version: api::MinimumEvidenceVersion {
                namespace: observation.version.namespace,
                generation: observation.version.generation,
            },
            relation,
            bounds,
        });
    }
    // Structural admission and Arc publication use this same persistent payer.
    // No scalar is queried during validation; its bound covers sparse-key sets.
    pools
        .symbolic
        .charge((pools.symbolic.usage.work as usize).saturating_mul(2))?;
    let model = api::MinimumModel {
        namespace: api::AnalysisNamespace::Baseline,
        scope: input.scope,
        fork: input.request.fork,
        source_snapshot: hybrid::snapshot(native.snapshot())?,
        revision: api::DecimalU64(native.revision().value()),
        root_binding: mapping::root_binding(native.root_binding()),
        declared_root_source: process::source(declared_root_source),
        entry_address: native.entry().address.to_string(),
        fingerprint,
        status: api::AnalysisStatus::Converged,
        current: source.coverage().baseline_current,
        config: mapping::execution_config(native.config()),
        numeric_model: input.numeric_model,
        domain_spec: super::terminal::domain_spec(native.domain_spec()),
        environment,
        accounts,
        summaries: output,
        postprocessing_max_work,
        export_limits: pools.symbolic.limits,
        export_usage: pools.symbolic.usage,
    };
    model.validate().map_err(|failure| api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: failure.to_string(),
        details: api::ErrorDetails::Report(failure),
    })?;
    pools.symbolic.charge(0)?;
    // No private array, path, history or SSA root can enter this profile.
    debug_assert!(pools.symbolic.graph.nodes.is_empty());
    let programs = pools.programs;
    let minimum = Some(Arc::new(model));
    drop(pools.symbolic);
    checkpoint(control)?;
    Ok(api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        selected_view: api::AnalysisNamespace::Baseline,
        status: api::AnalysisStatus::Converged,
        work: source.work() as u64,
        transfers: native.transfers() as u64,
        postprocessing_work: budget.used() as u64,
        timings: api::AnalysisTimings::default(),
        acquisition: None,
        programs,
        minimum,
        rich_export: api::RichExportOutcome::NotRequested,
        global: None,
        native: None,
        hybrid: Some(api::HybridReport {
            coverage,
            local_prefixes: Vec::new(),
            precision: None,
            rpc: None,
        }),
    })
}

fn accounts<N: WordElement>(
    native: &analysis::EvmWorldAnalysis<N>,
    pools: &mut Pools<'_, N>,
) -> Result<Vec<api::MinimumInputAccount>, api::ApiError> {
    pools
        .symbolic
        .charge(native.world().accounts().len().saturating_mul(64))?;
    let mut rows = Vec::with_capacity(native.world().accounts().len());
    for (address, account) in native.world().accounts() {
        let root_template = pools.root_namespace == Some(*address);
        let code = numeric::code(
            *address,
            (!root_template).then_some(*address),
            &account.code,
            pools,
        )?;
        pools
            .symbolic
            .charge(account.storage.len().saturating_mul(72))?;
        let mut storage = Vec::with_capacity(account.storage.len());
        for (slot, value) in &account.storage {
            storage.push(api::StorageEntry {
                slot: value::word(*slot),
                value: pools.symbolic.value(value)?,
            });
        }
        rows.push(api::MinimumInputAccount {
            address: address.to_string(),
            root_template,
            existence: match account.existence {
                evm_abstract::world::Existence::Present => api::AccountExistence::Present,
                evm_abstract::world::Existence::Absent => api::AccountExistence::Absent,
                evm_abstract::world::Existence::Unknown => api::AccountExistence::Unknown,
            },
            balance: pools.symbolic.value(&account.balance)?,
            nonce: pools.symbolic.value(&account.nonce)?,
            code_kind: code.kind,
            program: code.program,
            delegation_target: code.delegation_target,
            storage_unknown: account.storage_unknown,
            storage,
        });
    }
    Ok(rows)
}

struct ProjectedBounds<N: WordElement> {
    source: analysis::EvmProcedureModels<N>,
    bounds: Arc<Vec<api::MinimumProcedureBounds>>,
}

fn bounds<N: WordElement>(
    observation: &analysis::EvmProcedureSummaryRecord<N>,
    tables: &mut Vec<ProjectedBounds<N>>,
    pools: &mut Pools<'_, N>,
) -> Result<Arc<Vec<api::MinimumProcedureBounds>>, api::ApiError> {
    pools.symbolic.charge(tables.len().saturating_add(4))?;
    if let Some(table) = tables
        .iter()
        .find(|table| table.source.shares_storage_with(&observation.procedures))
    {
        return Ok(Arc::clone(&table.bounds));
    }
    pools
        .symbolic
        .charge(observation.procedures.len().saturating_mul(16))?;
    let mut bounds = Vec::with_capacity(observation.procedures.len());
    for procedure in &observation.procedures {
        pools
            .symbolic
            .charge(procedure.outputs.len().saturating_mul(4))?;
        let mut outputs = Vec::with_capacity(procedure.outputs.len());
        for outcome in &procedure.outputs {
            outputs.push(api::MinimumOutput {
                kind: summaries::outcome(outcome.kind),
                generation: outcome.generation,
                data: numeric::bytes(&outcome.data, pools)?,
                store: numeric::store(&outcome.store, pools)?,
            });
        }
        bounds.push(api::MinimumProcedureBounds {
            version: api::MinimumProcedureVersion {
                namespace: procedure.version.namespace,
                entry_revision: procedure.version.entry_revision,
                output_revision: api::DecimalU64(procedure.version.output_revision),
                source_epoch: procedure
                    .version
                    .source_epoch
                    .map(|epoch| epoch.to_string()),
            },
            input: numeric::store(&procedure.input, pools)?,
            outputs,
        });
    }

    let bounds = Arc::new(bounds);
    pools.symbolic.charge(4)?;
    tables.push(ProjectedBounds {
        source: observation.procedures.clone(),
        bounds: Arc::clone(&bounds),
    });
    Ok(bounds)
}

fn invalid(kind: api::MinimumValidationKind) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: format!("invalid minimum publication: {kind:?}"),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Minimum(kind)),
    }
}

fn fingerprint<N: WordElement>(
    native: &analysis::EvmWorldAnalysis<N>,
    pools: &mut Pools<'_, N>,
) -> Result<String, api::ApiError> {
    // The established fingerprint serializes original observations twice, not
    // post-execution Stores. Pay the cached complete scalar serialization bound
    // before either pass; custom scalar serialization remains the native owner.
    for account in native.world().accounts().values() {
        pools
            .symbolic
            .charge(account.storage.len().saturating_add(2))?;
        for value in account
            .storage
            .values()
            .chain([&account.balance, &account.nonce])
        {
            pools
                .symbolic
                .charge(value.work_size().saturating_mul(512))?;
        }
        if let Code::Runtime(program) = &account.code {
            pools.symbolic.charge(program.byte_len())?;
        }
    }
    let fingerprint = native
        .world()
        .fingerprint()
        .map_err(crate::analyze::errors::fingerprint)?;
    pools.symbolic.charge(0)?;
    Ok(fingerprint.to_string())
}

fn environment<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &EvmEnvironmentValues<N>,
    algebra: &ValueAlgebra<D>,
    pools: &mut Pools<'_, N>,
) -> Result<api::MinimumEnvironment, api::ApiError> {
    // This is the only original byte-array projection. Procedure data and
    // effects are already immutable numerical rows and require no execution.
    pools
        .symbolic
        .charge(source.calldata.metadata_len().saturating_mul(4))?;
    pools
        .symbolic
        .charge(source.calldata.summary_projection_work(algebra.borrowed()))?;
    let calldata = source
        .calldata
        .summary_projection(algebra.borrowed())
        .map_err(|error| {
            let mut failure = pools.symbolic.failure(api::ExportFailureKind::StateRoot, 0);
            failure.message = format!("minimum input data projection failed: {error}");
            failure
        })?;
    pools.symbolic.charge(0)?;
    let mut hashes =
        |table: &std::collections::BTreeMap<evm_abstract::U256, alloy_primitives::B256>| {
            pools.symbolic.charge(table.len().saturating_mul(128))?;
            table
                .iter()
                .map(|(index, hash)| {
                    pools.symbolic.charge(0)?;
                    Ok(api::IndexedHash {
                        index: value::word(*index),
                        hash: hash.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, api::ApiError>>()
        };
    let block_hashes = hashes(&source.block_hashes)?;
    let blob_hashes = hashes(&source.blob_hashes.hashes)?;
    Ok(api::MinimumEnvironment {
        input_scope: source.input_scope_id().map(api::DecimalU64),
        to: value::address(source.to),
        caller: value::address(source.caller),
        origin: value::address(source.resolved_origin()),
        call_value: pools.symbolic.value(&source.value)?,
        calldata: numeric::bytes(&calldata, pools)?,
        is_static: source.is_static,
        gas_upper_bound: match source.gas {
            GasInput::Unknown => None,
            GasInput::UpperBound(value) => Some(value::word(value)),
        },
        gas_price: pools.symbolic.value(&source.gas_price)?,
        coinbase: value::address(source.coinbase),
        timestamp: pools.symbolic.value(&source.timestamp)?,
        number: pools.symbolic.value(&source.number)?,
        prevrandao: pools.symbolic.value(&source.prevrandao)?,
        gas_limit: pools.symbolic.value(&source.gas_limit)?,
        chain_id: if let Some(value) = &source.chain_id {
            pools.symbolic.value(value)?
        } else {
            pools.symbolic.charge(1024)?;
            value::observation(&WordObservation::top())
        },
        base_fee: pools.symbolic.value(&source.base_fee)?,
        blob_base_fee: pools.symbolic.value(&source.blob_base_fee)?,
        block_hashes,
        blob_hashes,
        blob_count: pools.symbolic.value(&source.blob_hashes.length)?,
    })
}
