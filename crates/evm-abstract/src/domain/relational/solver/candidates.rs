//! One immutable EVM encoding; each candidate keeps its independent native
//! allowance, original encoding limit and root charge-before-query order.

use super::{
    ExprId, Outcome, QueryReason, RelationLimits, RelationState, prepared_prefix, unknown, word,
};
use crate::domain::relational::{CandidateError, CheckResult};
use alloy_primitives::U256;
use embedded_smt::{BudgetedOutcome, QueryBudget};

const CHUNK: usize = 32;

pub(crate) fn check(
    state: &RelationState,
    expression: &ExprId,
    candidates: &[U256],
    limits: &RelationLimits,
    budget: &mut QueryBudget<'_>,
) -> Result<Vec<CheckResult>, CandidateError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    // The old transfer charges before entering can_equal/prepared. A zero
    // balance must therefore win over an expression/configuration failure.
    if !budget.reserve() {
        return Err(CandidateError::Work);
    }
    if state.bottom {
        let mut results = Vec::new();
        for _ in candidates {
            if !budget.consume() {
                return Err(CandidateError::Work);
            }
            results.push(CheckResult::Unsat);
        }
        return Ok(results);
    }
    let encoded = (|| {
        let (base, mut encoder) = prepared_prefix(state, limits)?;
        // This is the charge of the original extra Constraint::Equal, before
        // encoding its target. Candidate constants remain individually accounted.
        encoder.charge(1)?;
        let value = encoder.expression(expression)?;
        Ok::<_, QueryReason>((base, encoder, value))
    })();
    let (base, mut encoder, value) = match encoded {
        Ok(encoded) => encoded,
        Err(reason) => {
            // The original charged operation has completed with an encoding
            // failure. Reusing this lease must not reuse its paid permit.
            assert!(budget.consume(), "encoding consumes its reserved query");
            return Err(CandidateError::Query(reason));
        }
    };
    let target_work = encoder.work;
    let mut after = Vec::new();
    encoder.emit_definitions(&mut after);
    let mut answers = Vec::new();
    for chunk in candidates.chunks(CHUNK) {
        let mut literals = Vec::new();
        let mut refused_encoding = false;
        for candidate in chunk {
            // Every original query starts from the SAME post-target memo. Do
            // not insert earlier candidates here and silently lower max_nodes.
            let missing = !encoder.memo.contains_key(&ExprId::constant(*candidate));
            if target_work.saturating_add(usize::from(missing)) > limits.max_nodes {
                refused_encoding = true;
                break;
            }
            literals.push(word(*candidate));
        }
        if !literals.is_empty() {
            let results = embedded_smt::check_candidates_budgeted(
                &base,
                &value,
                &literals,
                &after,
                limits.provider,
                limits.rlimit,
                budget,
            );
            for result in results {
                answers.push(match result {
                    BudgetedOutcome::Work => return Err(CandidateError::Work),
                    BudgetedOutcome::Solver(Outcome::Sat(_)) => CheckResult::Sat,
                    BudgetedOutcome::Solver(Outcome::Unsat) => CheckResult::Unsat,
                    BudgetedOutcome::Solver(Outcome::Unknown(reason)) => {
                        return Err(CandidateError::Query(unknown(reason, limits.provider)));
                    }
                    BudgetedOutcome::Solver(Outcome::Error(error)) => {
                        return Err(CandidateError::Query(QueryReason::SolverError(error)));
                    }
                });
            }
        }
        if refused_encoding {
            // Earlier candidates can stop at native Unknown/Work first. Only
            // after their successful prefix does this refused query get paid.
            if !budget.consume() {
                return Err(CandidateError::Work);
            }
            if embedded_smt::current_cancellation().is_cancelled() {
                return Err(CandidateError::Query(QueryReason::Cancelled));
            }
            return Err(CandidateError::Query(QueryReason::ExpressionLimit));
        }
    }
    debug_assert_eq!(answers.len(), candidates.len());
    Ok(answers)
}

#[cfg(test)]
mod tests;
