//! Branch-local assumptions and bounded projection back into scalar values.
//!
//! Every native query reserves work from the transaction ledger. A resource or
//! encoding limit retains the path and records why relational precision stopped.

use super::{Execution, TransferContext, boundary};
use crate::{
    analysis::{FrontierReason, MachinePayload},
    domain::{
        AbstractValue, NumericValue,
        relational::{CheckResult, RelationState, ScalarQuery, ValueQuery},
        symbolic::ExprId,
    },
};

fn charge(
    state: &RelationState,
    expression: &ExprId,
    checks: usize,
    context: &mut TransferContext<'_>,
) -> bool {
    let limits = context.config.analysis.relations;
    context.budget.charge(
        state
            .work_size()
            .saturating_add(expression.work_size())
            .saturating_add((limits.rlimit as usize).saturating_mul(checks)),
    )
}

/// Project only proven consequences; the expression and its scope survive.
pub(super) fn refine(
    value: &mut AbstractValue,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    let limits = context.config.analysis.relations;
    if !limits.enabled || relations.is_empty() || value.singleton().is_some() {
        return Ok(!relations.is_bottom());
    }
    let Some(expression) = value.expression().cloned() else {
        return Ok(true);
    };
    // Constraints on another independent input cannot make this value unique.
    if !context
        .budget
        .charge(relations.work_size().saturating_add(expression.work_size()))
    {
        return Err(FrontierReason::Work);
    }
    if !relations.relevant(&expression) {
        return Ok(true);
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
                return Ok(true);
            }
        }
        ScalarQuery::Infeasible => return Ok(false),
        ScalarQuery::Unchanged => {}
        ScalarQuery::Unknown(crate::domain::relational::QueryReason::ScalarFactLimit) => {}
        ScalarQuery::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
    }
    if !charge(relations, &expression, 2, context) {
        return Err(FrontierReason::Work);
    }
    match relations.unique_value(&expression, &limits) {
        ValueQuery::Unique(exact) => {
            if value.contains(exact) {
                *value = value.clone().with_numeric(NumericValue::constant(exact));
            } else {
                return Ok(false);
            }
        }
        ValueQuery::Multiple => {
            if let Some(candidates) = value.constants().cloned() {
                let mut retained = Vec::new();
                for candidate in candidates {
                    if !charge(relations, &expression, 1, context) {
                        return Err(FrontierReason::Work);
                    }
                    match relations.can_equal(&expression, candidate, &limits) {
                        CheckResult::Unsat => {}
                        CheckResult::Sat => retained.push(candidate),
                        CheckResult::Unknown(reason) => {
                            return Err(FrontierReason::Relations(reason));
                        }
                    }
                }
                if let Some(narrowed) = retained
                    .into_iter()
                    .map(AbstractValue::constant)
                    .reduce(|left, right| context.domain.join(&left, &right))
                {
                    *value = value.clone().with_numeric(narrowed.numeric().clone());
                } else {
                    return Ok(false);
                }
            }
        }
        ValueQuery::Infeasible => return Ok(false),
        ValueQuery::Unknown(reason) => return Err(FrontierReason::Relations(reason)),
    }
    Ok(true)
}

/// Establish a new runtime value without confusing separate executions of a PC.
pub(super) fn identify(value: AbstractValue, enabled: bool) -> AbstractValue {
    if enabled
        && value.singleton().is_none()
        && value.expression().is_none()
        && let Some(expression) = ExprId::fresh()
    {
        return value.with_expression(expression);
    }
    value
}

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
    if !charge(&payload.relations, &expression, 1, context) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    match payload.relations.assume(&expression, nonzero, &limits) {
        CheckResult::Unsat => return None,
        CheckResult::Sat => {}
        CheckResult::Unknown(reason) => boundary(result, pc, FrontierReason::Relations(reason)),
    }
    let relations = payload.relations.clone();
    for value in &mut payload.active_mut().stack {
        match refine(value, &relations, context) {
            Ok(false) => return None,
            Ok(true) => {}
            Err(reason) => {
                let stop = reason == FrontierReason::Work;
                boundary(result, pc, reason);
                if stop {
                    return None;
                }
            }
        }
    }
    match refine(&mut payload.active_mut().call_value, &relations, context) {
        Ok(false) => return None,
        Ok(true) => {}
        Err(reason) => {
            let stop = reason == FrontierReason::Work;
            boundary(result, pc, reason);
            if stop {
                return None;
            }
        }
    }
    Some(payload)
}
