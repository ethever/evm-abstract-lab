//! Complete finite projection with the transaction's original per-query charge.

use super::TransferContext;
use crate::{
    analysis::FrontierReason,
    domain::{
        AbstractValue,
        relational::{CandidateError, CheckResult, RelationState},
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
    let candidates = candidates.iter().copied().collect::<Vec<_>>();
    let cost = relations
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(limits.rlimit as usize);
    let answers = {
        let mut budget = context
            .budget
            .cached_query_budget(cost, limits.cache_scope());
        relations.can_equal_candidates(expression, &candidates, &limits, &mut budget)
    }
    .map_err(|reason| match reason {
        CandidateError::Work => FrontierReason::Work,
        CandidateError::Query(reason) => FrontierReason::Relations(reason),
    })?;
    assert_eq!(
        answers.len(),
        candidates.len(),
        "a successful batch is complete"
    );
    let mut retained = Vec::new();
    for (candidate, answer) in candidates.into_iter().zip(answers) {
        match answer {
            CheckResult::Unsat => {}
            CheckResult::Sat => retained.push(candidate),
            CheckResult::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
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
