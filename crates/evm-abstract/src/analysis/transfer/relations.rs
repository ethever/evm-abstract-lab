//! Branch-local assumptions and bounded projection back into scalar values.
//!
//! Local propagation reserves its bounded work from the transaction ledger.
//! Real bounds retain the path and record why optional precision stopped.

mod cache;
mod state;
#[cfg(test)]
mod tests;

use super::{Execution, TransferContext, boundary};
use crate::{
    analysis::{FrontierReason, MachinePayload},
    domain::{
        AbstractValue, NumericValue,
        relational::{CheckResult, QueryReason, RelationState, ScalarQuery, ValueQuery},
        symbolic::ExprId,
    },
};

pub(super) use cache::QueryCache;

/// Failure of an optional value projection does not leave an execution path
/// unexpanded. Cancellation and the shared ledger still stop real work.
pub(super) fn refinement_failure(
    result: &mut Execution,
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
pub(super) fn refine(
    value: &mut AbstractValue,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
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
fn prepare_refine(
    value: &mut AbstractValue,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<PreparedRefinement, FrontierReason> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    if !context.budget.charge(0) {
        return Err(FrontierReason::Work);
    }
    let limits = context.config.analysis.relations;
    if !limits.enabled || value.singleton().is_some() {
        return Ok(PreparedRefinement::Done(!relations.is_bottom()));
    }
    let Some(expression) = value.expression().cloned() else {
        return Ok(PreparedRefinement::Done(true));
    };
    let has_state_semantics = expression.has_state_semantics();
    if relations.is_empty() && !has_state_semantics {
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
    if !context
        .budget
        .charge(context.domain.refinement_work(value, relations.len()))
    {
        return Err(FrontierReason::Work);
    }
    match relations.refine_numeric(&expression, value.numeric(), context.domain, &limits) {
        ScalarQuery::Refined { numeric, complete } => {
            *value = value.clone().with_numeric(*numeric);
            if complete || value.singleton().is_some() {
                return Ok(PreparedRefinement::Done(true));
            }
        }
        ScalarQuery::Infeasible => return Ok(PreparedRefinement::Done(false)),
        ScalarQuery::Unchanged => {}
        ScalarQuery::Unknown(crate::domain::relational::QueryReason::ScalarFactLimit) => {}
        ScalarQuery::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
    }
    let cache_work = context.query_cache.work(relations, &expression);
    if cache_work == usize::MAX || !context.budget.charge(cache_work) {
        return Err(FrontierReason::Work);
    }
    if let Some(answer) = context.query_cache.get(relations, &expression, &limits) {
        return finish_refine(value, context, answer).map(PreparedRefinement::Done);
    }
    Ok(PreparedRefinement::Pending(expression))
}

/// Bounded local propagation; the cache contains only this frozen conjunction.
fn local_refine(
    value: &mut AbstractValue,
    expression: &ExprId,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    let limits = context.config.analysis.relations;
    let answer = relations.local_value_budgeted(expression, &limits, context.budget);
    if context.budget.exhausted() {
        return Err(FrontierReason::Work);
    }
    context
        .query_cache
        .remember(relations, expression, &limits, &answer);
    finish_refine(value, context, answer)
}

/// Apply each proof to this caller's own numeric upper bound and identity.
fn finish_refine(
    value: &mut AbstractValue,
    context: &mut TransferContext<'_>,
    answer: ValueQuery,
) -> Result<bool, FrontierReason> {
    match answer {
        ValueQuery::Exact(exact) => {
            if value.contains(exact) {
                *value = value.clone().with_numeric(NumericValue::constant(exact));
            } else {
                return Ok(false);
            }
        }
        ValueQuery::Bounds(bounds) => {
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
            if bounds.is_top() || value.numeric() == bounds.as_ref() {
                return Ok(true);
            }
            if !context
                .budget
                .charge(context.domain.refinement_work(value, 1))
            {
                return Err(FrontierReason::Work);
            }
            let Some(numeric) = crate::domain::relational::intersect_numeric(
                value.numeric(),
                &bounds,
                context.domain,
            ) else {
                return Ok(false);
            };
            *value = value.clone().with_numeric(numeric);
        }
        ValueQuery::ProvenImpossible => return Ok(false),
        ValueQuery::Undetermined(reason) => return Err(FrontierReason::Relations(reason)),
    }
    Ok(true)
}

pub(super) use crate::analysis::state::identify;

/// Each successor owns its assumptions; the predecessor remains unchanged.
pub(super) fn branch(
    result: &mut Execution,
    condition: &AbstractValue,
    nonzero: bool,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> Option<MachinePayload> {
    if !context.budget.charge(result.payload.work_size()) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let mut payload = result.payload.clone();
    let limits = context.config.analysis.relations;
    if !limits.enabled {
        return Some(payload);
    }
    // The numerical layer has already selected the only feasible edge. Its
    // constant condition supplies no additional relational information.
    if condition.singleton().is_some() {
        return Some(payload);
    }
    let Some(expression) = condition.symbolic_expression() else {
        return Some(payload);
    };
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
    let checked =
        payload
            .relations
            .assume_checked_budgeted(&expression, nonzero, &limits, context.budget);
    if context.budget.exhausted() {
        boundary(result, pc, FrontierReason::Work);
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
    Some(payload)
}
