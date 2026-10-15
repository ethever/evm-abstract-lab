//! Freeze the entire relational conjunction and all target definitions before
//! the joint exclusion algorithm runs. Single-expression cache keys stay separate.

use super::{ExprId, QueryReason, RelationLimits, RelationState, prepared_prefix, unknown};
use crate::domain::relational::{BudgetedValueQuery, ValueQuery};
use alloy_primitives::U256;
use embedded_smt::{BudgetedUniqueOutcome, QueryBudget, UniqueOutcome};

pub(crate) fn unique(
    state: &RelationState,
    expressions: &[ExprId],
    limits: &RelationLimits,
    budget: &mut QueryBudget<'_>,
) -> Result<Vec<BudgetedValueQuery>, QueryReason> {
    if expressions.len() < 2 {
        return Err(QueryReason::Configuration);
    }
    if expressions.len() > embedded_smt::MAX_UNIQUE_TARGETS {
        return Err(QueryReason::ExpressionLimit);
    }
    // Refusing an unpaid query wins over encoding/configuration/cancellation
    // failures, as on the existing per-expression path. Hold this permit until
    // phase1 consumes it, so successful preparation never charges twice.
    if !budget.reserve() {
        return Ok(vec![BudgetedValueQuery::Work; expressions.len()]);
    }
    let prepared = (|| {
        // Comparison and temporary storage are bounded by the admitted count
        // and occur only after reserving the preparation/query fee.
        for (index, expression) in expressions.iter().enumerate() {
            if expressions[..index].contains(expression) {
                return Err(QueryReason::Configuration);
            }
        }
        if state.bottom {
            return Ok(None);
        }
        let (mut assertions, mut encoder) = prepared_prefix(state, limits)?;
        let targets = expressions
            .iter()
            .map(|expression| encoder.expression(expression))
            .collect::<Result<Vec<_>, _>>()?;
        // State reads, UF declarations, leaf bindings and every intermediate
        // remain in the same Encoder. No definition is added after freezing F.
        encoder.emit_definitions(&mut assertions);
        Ok(Some((assertions, targets)))
    })();
    let (assertions, targets) = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => {
            assert!(budget.consume(), "bottom consumes its reserved query");
            return Ok(vec![
                BudgetedValueQuery::Value(ValueQuery::Infeasible);
                expressions.len()
            ]);
        }
        Err(reason) => {
            // Paid preparation is never refunded or reused by fallback.
            assert!(budget.consume(), "encoding consumes its reserved query");
            return Err(reason);
        }
    };
    embedded_smt::check_unique_many_budgeted(
        &assertions,
        &targets,
        limits.provider,
        limits.rlimit,
        budget,
    )
    .map(|answers| {
        answers
            .into_iter()
            .map(|answer| match answer {
                BudgetedUniqueOutcome::Work => BudgetedValueQuery::Work,
                BudgetedUniqueOutcome::Solver(answer) => {
                    BudgetedValueQuery::Value(value_query(answer, limits.provider))
                }
            })
            .collect()
    })
    // EVM targets have fixed Word256 width. Any combined packing admission
    // failure loses only joint precision and must retry the ordinary queries.
    .map_err(|_| QueryReason::ExpressionLimit)
}

pub(super) fn value_query(
    answer: UniqueOutcome,
    provider: super::super::SmtProvider,
) -> ValueQuery {
    match answer {
        UniqueOutcome::Unique(printed) => {
            let constant = if let Some(hex) = printed.strip_prefix("#x") {
                U256::from_str_radix(hex, 16).ok()
            } else if let Some(binary) = printed.strip_prefix("#b") {
                U256::from_str_radix(binary, 2).ok()
            } else {
                None
            };
            constant.map_or(
                ValueQuery::Unknown(QueryReason::ModelUnavailable),
                ValueQuery::Unique,
            )
        }
        UniqueOutcome::Multiple => ValueQuery::Multiple,
        UniqueOutcome::Infeasible => ValueQuery::Infeasible,
        UniqueOutcome::ModelUnavailable => ValueQuery::Unknown(QueryReason::ModelUnavailable),
        UniqueOutcome::Unknown(reason) => ValueQuery::Unknown(unknown(reason, provider)),
        UniqueOutcome::Error(error) => ValueQuery::Unknown(QueryReason::SolverError(error)),
    }
}

#[cfg(test)]
mod tests;
