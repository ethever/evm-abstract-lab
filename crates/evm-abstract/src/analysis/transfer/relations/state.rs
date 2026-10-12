//! Preserve branch proofs in numeric views of conditional byte and slot roots.

use super::super::{TransferContext, errors};
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        AbstractValue,
        relational::{QueryReason, RelationState},
        symbolic::ExprId,
    },
    world::{GuardRefinementError, Snapshot},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

fn failure(error: GuardRefinementError<FrontierReason>) -> FrontierReason {
    match error {
        GuardRefinementError::Bytes(error) => errors::bytes(error),
        GuardRefinementError::Store(error) => errors::store(error),
        GuardRefinementError::Facts(reason) => FrontierReason::Relations(reason),
        GuardRefinementError::Proof(reason) => reason,
    }
}

pub(super) fn refine(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    // Pay for metadata traversal and checkpoint copies before doing either.
    // World refinement visits retained nodes, never the logical COPY length.
    if !context.budget.charge(payload.work_size().saturating_mul(2)) {
        return Err(FrontierReason::Work);
    }
    let domain = context.domain;
    let limits = context.config.analysis.relations.expressions();
    let mut known = BTreeMap::new();
    let mut infeasible = false;
    let mut proof = |condition: &ExprId| -> Result<Option<bool>, FrontierReason> {
        if let Some(truth) = known.get(condition) {
            return Ok(*truth);
        }
        // A nonzero Word256 need not be 1. Ask for the unique boolean ISZERO
        // result rather than assigning a fabricated concrete selector value.
        let zero = ExprId::operation(opcode::ISZERO, std::slice::from_ref(condition), limits)
            .map_err(errors::expression)?;
        let mut value = AbstractValue::unsigned_range(U256::ZERO, U256::ONE)
            .expect("nonempty boolean range")
            .with_expression(zero);
        let truth = match super::refine(&mut value, relations, context) {
            Ok(false) => {
                infeasible = true;
                None
            }
            Ok(true) => value.singleton().map(|zero| zero == U256::ZERO),
            Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                None
            }
            Err(reason) => return Err(reason),
        };
        known.insert(condition.clone(), truth);
        Ok(truth)
    };
    if !payload
        .store
        .refine_guards(domain, limits, &mut proof)
        .map_err(failure)?
    {
        return Ok(false);
    }
    for frame in payload.call_stack.iter_mut() {
        for bytes in [
            &mut frame.memory,
            &mut frame.calldata,
            &mut frame.returndata,
        ] {
            if !bytes
                .refine_guards(domain, limits, &mut proof)
                .map_err(failure)?
            {
                return Ok(false);
            }
        }
        let mut saved = frame.saved_store.state().clone();
        if !saved
            .refine_guards(domain, limits, &mut proof)
            .map_err(failure)?
        {
            return Ok(false);
        }
        frame.saved_store = Snapshot::from_state(saved);
    }
    Ok(!infeasible)
}
