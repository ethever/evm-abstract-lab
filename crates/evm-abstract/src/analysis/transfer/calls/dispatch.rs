//! Guarded target partitioning leaves caller roots and their identities intact.

use super::{AbstractValue, Execution, FrontierReason, MachinePayload, TransferContext, boundary};
use crate::{
    analysis::DiagnosticKind,
    domain::{
        NumericValue,
        relational::{CheckResult, QueryReason},
    },
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) fn target_guard(
    result: &mut Execution,
    payload: &MachinePayload,
    projected: &AbstractValue,
    candidate: &AbstractValue,
    equal: bool,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> Option<MachinePayload> {
    let operands = [projected.clone(), candidate.clone()];
    if !context
        .budget
        .charge(context.domain.operation_work(&operands))
    {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let condition = context.domain.apply(opcode::EQ, &operands);
    if equal && !condition.may_be_nonzero() || !equal && !condition.may_be_zero() {
        return None;
    }
    if !context.budget.charge(payload.work_size()) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let mut guarded = payload.clone();
    let limits = context.config.analysis.relations;
    if !limits.enabled || condition.singleton().is_some() {
        return Some(guarded);
    }
    let Some(expression) = condition.symbolic_expression() else {
        return Some(guarded);
    };
    let observed = guarded
        .active()
        .stack
        .iter()
        .chain([&guarded.active().call_value, &condition])
        .cloned()
        .collect::<Vec<_>>();
    for value in observed {
        if let Some(expression) = value.expression()
            && let Err(reason) = guarded
                .relations
                .observe(expression, value.numeric(), &limits)
        {
            boundary(result, pc, FrontierReason::Relations(reason));
        }
    }
    let work = guarded
        .relations
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(if equal { limits.rlimit as usize } else { 0 });
    if !context.budget.charge(work) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    if !equal {
        // EQ == 0 is the exact remainder partition, independent of whether its
        // feasibility is checked now. Retain it through the bounded scalar
        // interface, avoiding a native query for each cumulative exclusion.
        // Later positive candidates and actual reads still consult this state.
        if let Err(reason) =
            guarded
                .relations
                .observe(&expression, &NumericValue::constant(U256::ZERO), &limits)
        {
            boundary(result, pc, FrontierReason::Relations(reason));
        }
        return Some(guarded);
    }
    match guarded.relations.assume_checked(&expression, true, &limits) {
        Ok(CheckResult::Unsat) => return None,
        Ok(CheckResult::Sat) => {}
        Ok(CheckResult::Unknown(QueryReason::Cancelled)) | Err(QueryReason::Cancelled) => {
            boundary(
                result,
                pc,
                FrontierReason::Relations(QueryReason::Cancelled),
            );
            return None;
        }
        Err(reason) => boundary(result, pc, FrontierReason::Relations(reason)),
        Ok(CheckResult::Unknown(reason)) => result
            .diagnostics
            .push((pc, DiagnosticKind::RelationalPrecisionLost(reason))),
    }
    // Dispatch adds a guard without changing any caller value or content root.
    // Their previous numeric bounds remain sound for this subset. Reprojecting
    // every array for every excluded native address repeats unrelated work;
    // subsequent actual reads and ordinary branches refine using these same
    // retained EQ/NE assumptions and original input expressions.
    Some(guarded)
}
