//! Refine state wrappers from locally proved guard values.
//!
//! An unproved guard keeps both historical alternatives. The callback memoizes
//! only within the same immutable relation state and never changes a guard.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::super::super::errors;
use super::super::TransferContext;

use crate::{
    analysis::{DiagnosticKind, EvmMachinePayload, FrontierReason},
    domain::{
        AlgebraRef, EvmValue,
        relational::{EvmScalarQuery, QueryReason, RelationState},
        symbolic::{ExprId, ExprLimits},
    },
    world::{EvmSnapshot, GuardRefinementError},
};
use std::collections::BTreeMap;

fn failure(error: GuardRefinementError<FrontierReason>) -> FrontierReason {
    match error {
        GuardRefinementError::Bytes(error) => errors::bytes(error),
        GuardRefinementError::Store(error) => errors::store(error),
        GuardRefinementError::Facts(reason) => FrontierReason::Relations(reason),
        GuardRefinementError::Proof(reason) => reason,
    }
}

pub(super) fn refine<D: EvmNumericAlgebra>(
    payload: &mut EvmMachinePayload<D::Element>,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
    policy: (AlgebraRef<'_, D>, ExprLimits),
) -> Result<bool, FrontierReason> {
    apply(payload, relations, context, diagnostics, pc, policy)
}

fn apply<D: EvmNumericAlgebra>(
    payload: &mut EvmMachinePayload<D::Element>,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
    policy: (AlgebraRef<'_, D>, ExprLimits),
) -> Result<bool, FrontierReason> {
    let (domain, limits) = policy;
    let mut known = BTreeMap::new();
    let mut infeasible = false;
    let mut proof = |condition: &ExprId| -> Result<Option<bool>, FrontierReason> {
        if let Some(truth) = known.get(condition) {
            return Ok(*truth);
        }
        // The same bounded local refinement serves every wrapper. Repeated
        // conditions share an answer only within this frozen relation state.
        if !context
            .budget
            .charge(relations.work_size().saturating_add(condition.work_size()))
        {
            return Err(FrontierReason::Work);
        }
        let value = EvmValue::<D::Element>::top();
        let truth = match relations
            .refine_numeric_budgeted_with(
                condition,
                &value,
                domain,
                &context.config.analysis.relations,
                context.budget,
            )
            .map_err(|_| FrontierReason::Work)?
        {
            EvmScalarQuery::Refined { numeric, .. } => {
                if !numeric.may_be_zero() {
                    Some(true)
                } else if !numeric.may_be_nonzero() {
                    Some(false)
                } else {
                    None
                }
            }
            EvmScalarQuery::Infeasible => {
                infeasible = true;
                None
            }
            EvmScalarQuery::Unchanged => None,
            EvmScalarQuery::Unknown(reason) if reason != QueryReason::Cancelled => {
                diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                None
            }
            EvmScalarQuery::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
        };
        known.insert(condition.clone(), truth);
        Ok(truth)
    };
    if !payload
        .store
        .refine_guards_with(domain, limits, &mut proof)
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
                .refine_guards_with(domain, limits, &mut proof)
                .map_err(failure)?
            {
                return Ok(false);
            }
        }
        let mut saved = frame.saved_store.state().clone();
        if !saved
            .refine_guards_with(domain, limits, &mut proof)
            .map_err(failure)?
        {
            return Ok(false);
        }
        frame.saved_store = EvmSnapshot::<D::Element>::from_state(saved);
    }
    Ok(!infeasible)
}

#[cfg(test)]
mod tests;
