//! Bounded projection of procedure provenance without expanding callback graphs.
mod regional;
#[cfg(test)]
mod tests;

use crate::analyze::{checkpoint, pools, process, symbolic::Pool};
use evm_abstract::{
    analysis::{self, control::Control},
    domain::WordElement,
};
use evm_abstract_protocol as api;

pub(super) fn project<N: WordElement>(
    observations: &[analysis::EvmProcedureSummaryRecord<N>],
    control: &Control,
    symbolic: &mut Pool<'_>,
) -> Result<Vec<api::AbstractSummary>, api::ApiError> {
    let mut records = Vec::new();
    for record in observations {
        checkpoint(control)?;
        symbolic.charge(record.procedures.len().saturating_add(2))?;
        let mut procedures = Vec::new();
        for procedure in &record.procedures {
            checkpoint(control)?;
            if let Some(key) = procedure.frame.process.as_deref() {
                if process::key(key).source.semantics() != process::semantics(procedure.semantics) {
                    return Err(process::invalid(api::ProcessValidationKind::Identity));
                }
                if procedure.process_input.is_none() {
                    return Err(process::invalid(api::ProcessValidationKind::Operands));
                }
            } else if matches!(
                procedure.semantics,
                analysis::TransferSemantics::KnownProcess
                    | analysis::TransferSemantics::ConstructorProcess
                    | analysis::TransferSemantics::BoundaryProcess
            ) {
                return Err(process::invalid(api::ProcessValidationKind::Identity));
            }
            symbolic.charge(
                procedure
                    .covered_blocks
                    .len()
                    .saturating_add(procedure.outputs.len())
                    .saturating_add(procedure.dependencies.len())
                    .saturating_add(2),
            )?;
            procedures.push(api::AbstractProcedure {
                process: procedure.frame.process.as_deref().map(process::key),
                process_input: procedure
                    .process_input
                    .as_ref()
                    .map(|input| process::invocation(input, symbolic))
                    .transpose()?,
                semantics: process::semantics(procedure.semantics),
                code_address: procedure
                    .frame
                    .process
                    .is_none()
                    .then(|| procedure.frame.code_address.to_string()),
                code_hash: (procedure.frame.process.is_none()
                    && procedure.frame.mode != analysis::FrameCode::Unknown)
                    .then(|| procedure.frame.code_hash.to_string()),
                storage_address: procedure
                    .frame
                    .process
                    .is_none()
                    .then(|| procedure.frame.address.to_string()),
                kind: procedure.frame.process.as_deref().map_or_else(
                    || pools::mode(procedure.frame.mode),
                    |key| process::key(key).source.kind(),
                ),
                is_static: procedure
                    .frame
                    .process
                    .as_deref()
                    .map_or(procedure.frame.is_static, |key| key.is_static),
                covered_blocks: procedure.covered_blocks.clone(),
                outcomes: procedure
                    .outputs
                    .iter()
                    .map(|output| outcome(output.kind))
                    .collect(),
                dependencies: procedure.dependencies.clone(),
                input_updates: procedure.input_updates,
                output_updates: procedure.output_updates,
            });
        }
        let regional_cfg = regional::project(&record.regional_cfg, &procedures, symbolic)?;
        symbolic.charge(0)?;
        records.push(api::AbstractSummary {
            source_state: record.source_state,
            source_current: record.source_current,
            kind: match record.kind {
                analysis::ProcedureSummaryKind::KnownProcess => {
                    api::ProcedureSummaryKind::KnownProcess
                }
                analysis::ProcedureSummaryKind::UnknownEnvironment => {
                    api::ProcedureSummaryKind::UnknownEnvironment
                }
                analysis::ProcedureSummaryKind::KnownRecursion => {
                    api::ProcedureSummaryKind::KnownRecursion
                }
                analysis::ProcedureSummaryKind::KnownCall => api::ProcedureSummaryKind::KnownCall,
            },
            root_procedure: record.root_procedure,
            closed: record.closed,
            procedures,
            regional_cfg,
            state_count: record.state_count,
            transfers: record.transfers,
            propagations: record.propagations,
            reused: record.reused,
        });
    }
    symbolic.charge(0)?;
    Ok(records)
}

fn outcome(kind: analysis::OutcomeKind) -> api::OutcomeKind {
    match kind {
        analysis::OutcomeKind::Return => api::OutcomeKind::Return,
        analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
        analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
    }
}
