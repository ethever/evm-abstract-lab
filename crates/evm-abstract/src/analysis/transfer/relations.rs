//! Branch-local assumptions and bounded projection back into scalar values.
//!
//! Local propagation reserves its bounded work from the transaction ledger.
//! Real bounds retain the path and record why optional precision stopped.

use crate::domain::{EvmNumericAlgebra, WordElement};

mod cache;
mod state;
#[cfg(test)]
mod tests;

use super::{EvmExecution, TransferContext, boundary, query};
use crate::{
    analysis::{EvmMachinePayload, FrontierReason},
    domain::{
        EvmValue,
        relational::{CheckResult, EvmScalarQuery, EvmValueQuery, QueryReason, RelationState},
        symbolic::ExprId,
    },
};

pub(super) use cache::QueryCache;

/// Failure of an optional value projection does not leave an execution path
/// unexpanded. Cancellation and the shared ledger still stop real work.
pub(super) fn refinement_failure<N: WordElement>(
    result: &mut EvmExecution<N>,
    pc: usize,
    reason: FrontierReason,
) -> bool {
    match reason {
        FrontierReason::Relations(QueryReason::Cancelled) => {
            boundary(
                result,
                pc,
                FrontierReason::Relations(QueryReason::Cancelled),
            );
            true
        }
        FrontierReason::Relations(reason) => {
            result.diagnostics.push((
                pc,
                crate::analysis::DiagnosticKind::RelationalPrecisionLost(reason),
            ));
            false
        }
        reason => {
            boundary(result, pc, reason);
            true
        }
    }
}

/// Project only proven consequences; the expression and its scope survive.
pub(super) fn refine<D: EvmNumericAlgebra>(
    value: &mut EvmValue<D::Element>,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
) -> Result<bool, FrontierReason> {
    match prepare_refine(value, relations, context)? {
        PreparedRefinement::Done(feasible) => Ok(feasible),
        PreparedRefinement::Pending(expression) => {
            local_refine(value, &expression, relations, context)
        }
    }
}

enum PreparedRefinement {
    Done(bool),
    Pending(ExprId),
}

/// Keep the numerical and original per-value fact paths ahead of local evaluation.
/// Pending values have paid for local preparation, but not a local evaluation.
fn prepare_refine<D: EvmNumericAlgebra>(
    value: &mut EvmValue<D::Element>,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
) -> Result<PreparedRefinement, FrontierReason> {
    query::checkpoint(context.budget)?;
    let limits = context.domain.spec().relations();
    if !limits.enabled || relations.is_bottom() {
        return Ok(PreparedRefinement::Done(!relations.is_bottom()));
    }
    // Without a retained term there is no relational refinement to attempt.
    // In particular, consuming a literal in POP needs no numeric discovery.
    let Some(expression) = value.expression().cloned() else {
        return Ok(PreparedRefinement::Done(true));
    };
    let has_state_semantics = expression.has_state_semantics();
    if relations.is_empty() && !has_state_semantics {
        return Ok(PreparedRefinement::Done(true));
    }
    if query::singleton(value.numeric(), context.budget)?.is_some() {
        return Ok(PreparedRefinement::Done(true));
    }
    // Constraints on another independent input cannot make this value unique.
    if !context
        .budget
        .charge(relations.work_size().saturating_add(expression.work_size()))
    {
        return Err(FrontierReason::Work);
    }
    if !has_state_semantics && !relations.relevant(&expression) {
        return Ok(PreparedRefinement::Done(true));
    }
    let answer = relations.refine_numeric_budgeted(
        &expression,
        value,
        context.domain,
        &limits,
        context.budget,
    );
    query::checkpoint(context.budget)?;
    match answer.map_err(|_| FrontierReason::Work)? {
        EvmScalarQuery::Refined { numeric, complete } => {
            // The scalar envelope paid for import/reduction, not this caller's
            // query on the opaque result. Reuse its answer when installing it.
            let singleton = if !complete || value.symbolic_limit_reached() {
                query::singleton(numeric.as_ref(), context.budget)?
            } else {
                None
            };
            let refined = value
                .clone()
                .with_numeric_after_singleton(*numeric, singleton);
            query::checkpoint(context.budget)?;
            *value = refined;
            if complete || singleton.is_some() {
                return Ok(PreparedRefinement::Done(true));
            }
        }
        EvmScalarQuery::Infeasible => return Ok(PreparedRefinement::Done(false)),
        EvmScalarQuery::Unchanged => {}
        EvmScalarQuery::Unknown(crate::domain::relational::QueryReason::ScalarFactLimit) => {}
        EvmScalarQuery::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
    }
    if let Some(answer) = context
        .query_cache
        .get(relations, &expression, &limits, context.budget)
    {
        return finish_refine(value, context, answer).map(PreparedRefinement::Done);
    }
    Ok(PreparedRefinement::Pending(expression))
}

/// Bounded local propagation; the cache contains only this frozen conjunction.
fn local_refine<D: EvmNumericAlgebra>(
    value: &mut EvmValue<D::Element>,
    expression: &ExprId,
    relations: &RelationState,
    context: &mut TransferContext<'_, D>,
) -> Result<bool, FrontierReason> {
    let limits = context.domain.spec().relations();
    let answer =
        relations.local_value_budgeted(expression, &limits, context.budget, context.domain);
    query::checkpoint(context.budget)?;
    context
        .query_cache
        .remember(relations, expression, &limits, &answer, context.budget);
    query::checkpoint(context.budget)?;
    finish_refine(value, context, answer)
}

/// Apply each proof to this caller's own numeric upper bound and identity.
fn finish_refine<D: EvmNumericAlgebra>(
    value: &mut EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    answer: EvmValueQuery<D::Element>,
) -> Result<bool, FrontierReason> {
    match answer {
        EvmValueQuery::Exact(exact) => {
            if value.contains(exact) {
                let refined = value
                    .clone()
                    .with_numeric_after_singleton(D::Element::constant(exact), Some(exact));
                query::checkpoint(context.budget)?;
                *value = refined;
            } else {
                query::checkpoint(context.budget)?;
                return Ok(false);
            }
        }
        EvmValueQuery::Bounds(bounds) => {
            // Equality can traverse finite candidates. Pay before comparing, and
            // leave every value layer intact when the meet adds no numeric facts.
            if !context.budget.charge(
                value
                    .numeric()
                    .work_size()
                    .saturating_add(bounds.work_size()),
            ) {
                return Err(FrontierReason::Work);
            }
            let unchanged = bounds.is_top() || value.numeric() == bounds.as_ref();
            query::checkpoint(context.budget)?;
            if unchanged {
                return Ok(true);
            }
            let metadata_work = value
                .work_size()
                .saturating_sub(value.numeric().work_size());
            let numeric = context.domain.intersect_numeric_with_payment(
                value.numeric(),
                bounds.as_ref(),
                metadata_work,
                |work| {
                    query::checkpoint(context.budget)?;
                    if work != usize::MAX && context.budget.charge(work) {
                        query::checkpoint(context.budget)
                    } else {
                        Err(FrontierReason::Work)
                    }
                },
            );
            query::checkpoint(context.budget)?;
            let Some(numeric) = numeric? else {
                return Ok(false);
            };
            let singleton = if value.symbolic_limit_reached() {
                query::singleton(&numeric, context.budget)?
            } else {
                None
            };
            let refined = value
                .clone()
                .with_numeric_after_singleton(numeric, singleton);
            query::checkpoint(context.budget)?;
            *value = refined;
        }
        EvmValueQuery::ProvenImpossible => return Ok(false),
        EvmValueQuery::Undetermined(reason) => return Err(FrontierReason::Relations(reason)),
    }
    Ok(true)
}

pub(super) use crate::analysis::state::identify;

/// Each successor owns its assumptions; the predecessor remains unchanged.
#[cfg(test)]
pub(super) fn branch<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    condition: &EvmValue<D::Element>,
    nonzero: bool,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> Option<EvmMachinePayload<D::Element>> {
    branch_with_singleton(
        result,
        condition,
        nonzero,
        crate::domain::SingletonProof::NotQueried,
        context,
        pc,
    )
}

/// Dispatch classified this same immutable condition already. A saved answer
/// avoids querying again when adding assumptions to its two successor copies.
pub(super) fn branch_with_singleton<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    condition: &EvmValue<D::Element>,
    nonzero: bool,
    mut proof: crate::domain::SingletonProof,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> Option<EvmMachinePayload<D::Element>> {
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary(result, pc, reason);
        return None;
    }
    if !context.budget.charge(result.payload.work_size()) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let mut payload = result.payload.clone();
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary(result, pc, reason);
        return None;
    }
    let limits = context.domain.spec().relations();
    if !limits.enabled {
        return Some(payload);
    }
    let Some(expression) = condition.expression().cloned() else {
        return Some(payload);
    };
    // The numerical layer has already selected the only feasible edge. Its
    // constant condition supplies no additional relational information.
    let answer =
        if proof == crate::domain::SingletonProof::NotQueried && condition.constants().is_some() {
            // A complete cover permits conservative guard retention without a new
            // discovery. It does not assert that the full carrier is nonsingleton.
            Ok(None)
        } else {
            query::resolve(condition.numeric(), &mut proof, context.budget)
        };
    let singleton = match answer {
        Ok(singleton) => singleton,
        Err(reason) => {
            boundary(result, pc, reason);
            return None;
        }
    };
    if singleton.is_some() {
        return Some(payload);
    }
    let values = payload
        .active()
        .stack
        .iter()
        .chain([&payload.active().call_value, condition])
        .cloned()
        .collect::<Vec<_>>();
    for value in values {
        if let Some(expr) = value.expression()
            && let Err(reason) = payload.relations.observe(expr, value.numeric(), &limits)
        {
            boundary(result, pc, FrontierReason::Relations(reason));
        }
    }
    let query_fee = payload
        .relations
        .work_size()
        .saturating_add(expression.work_size());
    if !context.budget.charge(query_fee) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let checked = payload.relations.assume_checked_budgeted(
        &expression,
        nonzero,
        &limits,
        context.budget,
        context.domain,
    );
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary(result, pc, reason);
        return None;
    }
    let guard_retained = match checked {
        Ok(CheckResult::ProvenImpossible) => return None,
        Ok(CheckResult::MayBeFeasible) => true,
        Ok(CheckResult::Undetermined(QueryReason::Cancelled)) | Err(QueryReason::Cancelled) => {
            boundary(
                result,
                pc,
                FrontierReason::Relations(QueryReason::Cancelled),
            );
            return None;
        }
        Err(reason) => {
            boundary(result, pc, FrontierReason::Relations(reason));
            false
        }
        Ok(CheckResult::Undetermined(reason)) => {
            // The assumption is already retained. A failed feasibility check
            // loses optional pruning precision while both guarded edges survive.
            result.diagnostics.push((
                pc,
                crate::analysis::DiagnosticKind::RelationalPrecisionLost(reason),
            ));
            true
        }
    };
    let relations = payload.relations.clone();
    // Stack expressions retain the guard. Instruction operands and read results
    // are projected when consumed, avoiding a whole-conjunction pass per slot.
    let _ = guard_retained;
    match state::refine(
        &mut payload,
        &relations,
        context,
        &mut result.diagnostics,
        pc,
    ) {
        Ok(false) => return None,
        Ok(true) => {}
        Err(reason) => {
            if refinement_failure(result, pc, reason) {
                return None;
            }
        }
    }
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary(result, pc, reason);
        return None;
    }
    Some(payload)
}
