//! Optional consequences for the active frame's one returndata length.

use super::super::super::TransferContext;
use crate::{
    analysis::{FrontierReason, MachinePayload},
    domain::{
        NumericValue,
        relational::{QueryReason, RelationState, ValueQuery},
    },
    resource::WorkBudget,
};

// This is a private, prepaid work quantum, not a smaller numeric/relational
// policy. Its refusal must never exhaust the owner's execution ledger.
const QUERY_WORK: usize = 64 * 1024;

fn cancelled() -> Result<(), FrontierReason> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(super) fn refine(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<(), FrontierReason> {
    cancelled()?;
    let limits = context.config.analysis.relations;
    if !limits.enabled || !limits.valid() {
        return Ok(());
    }
    let frame = payload.active_mut();
    let length = frame.returndata.len();
    let Some(expression) = length.expression() else {
        return Ok(());
    };
    let preparation = length.work_size().saturating_add(1);
    if preparation > QUERY_WORK || !context.budget.can_afford(preparation) {
        cancelled()?;
        return Ok(());
    }
    if !context.budget.charge(preparation) {
        cancelled()?;
        return Ok(());
    }
    let mut proved = length.clone();
    if length.numeric().is_top() {
        if relations.is_empty() {
            return Ok(());
        }
        let work = context.query_cache.work(relations, expression);
        if work > QUERY_WORK || !context.budget.can_afford(work) {
            cancelled()?;
            return Ok(());
        }
        if !context.budget.charge(work) {
            cancelled()?;
            return Ok(());
        }
        let answer = if let Some(answer) = context.query_cache.get(relations, expression, &limits) {
            answer
        } else {
            if !context.budget.can_afford(QUERY_WORK) {
                cancelled()?;
                return Ok(());
            }
            if !context.budget.charge(QUERY_WORK) {
                cancelled()?;
                return Ok(());
            }
            // The owner pays the entire admitted quantum, including an
            // unsuccessful attempt. Never share a private refusal with it.
            let mut private = WorkBudget::new(QUERY_WORK);
            let answer = relations.local_value_budgeted(expression, &limits, &mut private);
            cancelled()?;
            // The private precision quantum is not part of the shared cache
            // identity. A refused optional query must not poison a later paid
            // query which may have enough of the owner's remaining budget.
            if private.exhausted() {
                return Ok(());
            }
            if !matches!(&answer, ValueQuery::Undetermined(_)) {
                context
                    .query_cache
                    .remember(relations, expression, &limits, &answer);
            }
            answer
        };
        cancelled()?;
        proved = match answer {
            ValueQuery::Exact(exact) => proved.with_numeric(NumericValue::constant(exact)),
            ValueQuery::Bounds(bounds) if !bounds.is_top() => proved.with_numeric(*bounds),
            ValueQuery::Undetermined(QueryReason::Cancelled) => {
                return Err(FrontierReason::Relations(QueryReason::Cancelled));
            }
            // This optional import never decides global feasibility.
            _ => return Ok(()),
        };
    }
    frame
        .memory
        .import_length_bound(&proved, limits.expressions(), context.budget)
        .map_err(FrontierReason::Relations)?;
    Ok(())
}
