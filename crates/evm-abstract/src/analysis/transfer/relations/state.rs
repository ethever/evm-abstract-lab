//! Preserve branch proofs in numeric views of conditional byte and slot roots.

use crate::domain::EvmNumericAlgebra;

mod guards;
mod length;

use super::super::TransferContext;
use crate::{
    analysis::{
        DiagnosticKind, EvmMachinePayload, FrontierReason, MachineError,
        state::projection::{Policy, Projection},
    },
    domain::relational::RelationState,
};

pub(super) fn refine<D: EvmNumericAlgebra>(
    payload: &mut EvmMachinePayload<D::Element>,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    // Pay for metadata traversal and checkpoint copies before doing either.
    // EvmWorld refinement visits retained nodes, never the logical COPY length.
    if !context.budget.charge(payload.work_size().saturating_mul(2)) {
        return Err(FrontierReason::Work);
    }
    let domain = context.domain;
    let limits = context.config.analysis.relations.expressions();
    let mut projection = Projection::new(
        relations,
        domain,
        &context.config.analysis.relations,
        context.budget,
        Policy::Branch { diagnostics, pc },
    );
    'metadata: for frame in payload.call_stack.iter_mut() {
        for value in [&mut frame.address_operand, &mut frame.caller_operand]
            .into_iter()
            .flatten()
        {
            projection.value_with(value);
            if projection.stopped() {
                break 'metadata;
            }
        }
        for bytes in frame.byte_arrays_mut() {
            projection.bytes(bytes);
            if projection.stopped() {
                break 'metadata;
            }
        }
    }
    if !projection.finish().map_err(MachineError::frontier_reason)? {
        return Ok(false);
    }
    length::refine(payload, relations, context)?;
    guards::refine(
        payload,
        relations,
        context,
        diagnostics,
        pc,
        (domain, limits),
    )
}
