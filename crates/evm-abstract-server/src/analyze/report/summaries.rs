//! Bounded projection of procedure provenance without expanding callback graphs.

use crate::analyze::{checkpoint, pools, symbolic::Pool};
use evm_abstract::analysis::{self, control::Control};
use evm_abstract_protocol as api;

pub(super) fn project(
    native: &analysis::WorldAnalysis,
    control: &Control,
    symbolic: &mut Pool<'_>,
) -> Result<Vec<api::AbstractSummary>, api::ApiError> {
    let mut records = Vec::new();
    for record in native.unknown_summaries() {
        checkpoint(control)?;
        symbolic.charge(record.procedures.len().saturating_add(1))?;
        let mut procedures = Vec::new();
        for procedure in &record.procedures {
            checkpoint(control)?;
            symbolic.charge(
                procedure
                    .covered_blocks
                    .len()
                    .saturating_add(procedure.outputs.len())
                    .saturating_add(procedure.dependencies.len())
                    .saturating_add(1),
            )?;
            procedures.push(api::AbstractProcedure {
                code_address: procedure.frame.code_address.to_string(),
                code_hash: (procedure.frame.mode != analysis::FrameCode::Unknown)
                    .then(|| procedure.frame.code_hash.to_string()),
                storage_address: procedure.frame.address.to_string(),
                kind: pools::mode(procedure.frame.mode),
                is_static: procedure.frame.is_static,
                covered_blocks: procedure.covered_blocks.clone(),
                outcomes: procedure
                    .outputs
                    .iter()
                    .map(|output| match output.kind {
                        analysis::OutcomeKind::Return => api::OutcomeKind::Return,
                        analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
                        analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
                    })
                    .collect(),
                dependencies: procedure.dependencies.clone(),
                input_updates: procedure.input_updates,
                output_updates: procedure.output_updates,
            });
        }
        records.push(api::AbstractSummary {
            source_state: record.source_state,
            closed: record.closed,
            procedures,
            state_count: record.state_count,
            transfers: record.transfers,
            propagations: record.propagations,
            reused: record.reused,
        });
    }
    Ok(records)
}
