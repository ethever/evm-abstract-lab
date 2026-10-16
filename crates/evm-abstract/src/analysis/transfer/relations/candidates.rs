//! Complete finite projection with the transaction's original per-query charge.

use super::TransferContext;
use crate::{
    analysis::FrontierReason,
    domain::{
        AbstractValue,
        relational::{CheckResult, RelationState},
        symbolic::ExprId,
    },
};
use alloy_primitives::U256;
use std::collections::BTreeSet;

pub(super) fn refine(
    value: &mut AbstractValue,
    expression: &ExprId,
    candidates: &BTreeSet<U256>,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    let limits = context.config.analysis.relations;
    let cost = relations
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(limits.max_nodes);
    let mut retained = Vec::new();
    for candidate in candidates {
        if !context.budget.charge(cost) {
            return Err(FrontierReason::Work);
        }
        let answer = relations.can_equal_budgeted(expression, *candidate, &limits, context.budget);
        if context.budget.exhausted() {
            return Err(FrontierReason::Work);
        }
        match answer {
            CheckResult::ProvenImpossible => {}
            CheckResult::MayBeFeasible => retained.push(*candidate),
            CheckResult::Undetermined(reason) => return Err(FrontierReason::Relations(reason)),
        }
    }
    if let Some(narrowed) = retained
        .into_iter()
        .map(AbstractValue::constant)
        .reduce(|left, right| context.domain.join(&left, &right))
    {
        *value = value.clone().with_numeric(narrowed.numeric().clone());
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
