//! Demand-local COPY minima under the same frozen branch relation state.

use super::super::super::{TransferContext, errors};
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        NumericValue,
        relational::{QueryReason, RelationState, ValueQuery, intersect_numeric},
    },
    resource::WorkBudget,
    world::GuardRefinementError,
};

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), FrontierReason> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    if budget.exhausted() || !budget.charge(work) {
        return Err(if analysis_control::current_cancellation().is_cancelled() {
            FrontierReason::Relations(QueryReason::Cancelled)
        } else {
            FrontierReason::Work
        });
    }
    Ok(())
}

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
    reserve(context.budget, 0)?;
    let limits = context.config.analysis.relations;
    if !limits.enabled || relations.is_empty() {
        return Ok(true);
    }
    let domain = context.domain;
    let cache = &mut context.query_cache;
    for frame in payload.call_stack.iter_mut() {
        for bytes in [
            &mut frame.memory,
            &mut frame.calldata,
            &mut frame.returndata,
        ] {
            if !bytes
                .refine_active_copy_lengths(
                    limits.expressions(),
                    context.budget,
                    reserve,
                    &mut |expression, old, budget| {
                        if old.singleton().is_some() {
                            return Ok(true);
                        }
                        reserve(
                            budget,
                            relations.work_size().saturating_add(expression.work_size()),
                        )?;
                        if !relations.relevant(expression) {
                            return Ok(true);
                        }
                        reserve(budget, cache.work(relations, expression))?;
                        let answer = if let Some(answer) = cache.get(relations, expression, &limits)
                        {
                            answer
                        } else {
                            let answer =
                                relations.local_value_budgeted(expression, &limits, budget);
                            // Cancellation takes priority over a refused work reservation.
                            reserve(budget, 0)?;
                            cache.remember(relations, expression, &limits, &answer);
                            answer
                        };
                        match answer {
                            ValueQuery::Exact(exact) => {
                                reserve(budget, domain.refinement_work(old, 1))?;
                                if !old.contains(exact) {
                                    return Ok(false);
                                }
                                *old = old.clone().with_numeric(NumericValue::constant(exact));
                            }
                            ValueQuery::Bounds(bounds) => {
                                reserve(
                                    budget,
                                    domain
                                        .refinement_work(old, 1)
                                        .saturating_add(bounds.work_size().saturating_mul(2)),
                                )?;
                                let Some(numeric) =
                                    intersect_numeric(old.numeric(), &bounds, domain)
                                else {
                                    return Ok(false);
                                };
                                *old = old.clone().with_numeric(numeric);
                            }
                            ValueQuery::ProvenImpossible => return Ok(false),
                            ValueQuery::Undetermined(QueryReason::Cancelled) => {
                                return Err(FrontierReason::Relations(QueryReason::Cancelled));
                            }
                            ValueQuery::Undetermined(reason) => {
                                diagnostics
                                    .push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                            }
                        }
                        Ok(true)
                    },
                )
                .map_err(failure)?
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
