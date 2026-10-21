//! Guarded target partitioning leaves caller roots and their identities intact.

use crate::domain::{EvmNumericAlgebra, SingletonProof, WordElement};

use super::{EvmExecution, EvmMachinePayload, EvmValue, FrontierReason, TransferContext, boundary};

use crate::{
    analysis::DiagnosticKind,
    domain::relational::{CheckResult, QueryReason},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) fn target_guard<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    payload: &EvmMachinePayload<D::Element>,
    projected: &EvmValue<D::Element>,
    candidate: &EvmValue<D::Element>,
    equal: bool,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> Option<EvmMachinePayload<D::Element>> {
    if !context.budget.charge(
        projected
            .clone_work()
            .saturating_add(candidate.clone_work()),
    ) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let operands = [projected.clone(), candidate.clone()];
    if !context
        .budget
        .charge(context.domain.operation_work(opcode::EQ, &operands))
    {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let (reduced, proof) = context
        .domain
        .apply_detailed_with_singleton(opcode::EQ, &operands);
    let condition = reduced.value;
    let truth = match super::Truth::prepare(&condition, proof, context.budget) {
        Ok(truth) => truth,
        Err(reason) => {
            boundary(result, pc, reason);
            return None;
        }
    };
    if equal && !truth.nonzero || !equal && !truth.zero {
        return None;
    }
    if !context.budget.charge(payload.work_size()) {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let mut guarded = payload.clone();
    let limits = context.domain.spec().relations();
    if !limits.enabled || matches!(truth.singleton, SingletonProof::Answer(Some(_))) {
        return Some(guarded);
    }
    let Some(expression) = condition.expression().cloned() else {
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
        .saturating_add(expression.work_size());
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
                .observe(&expression, &D::Element::constant(U256::ZERO), &limits)
        {
            boundary(result, pc, FrontierReason::Relations(reason));
        }
        return Some(guarded);
    }
    let answer = guarded.relations.assume_checked_budgeted(
        &expression,
        true,
        &limits,
        context.budget,
        context.domain,
    );
    if context.budget.exhausted() {
        boundary(result, pc, FrontierReason::Work);
        return None;
    }
    match answer {
        Ok(CheckResult::ProvenImpossible) => return None,
        Ok(CheckResult::MayBeFeasible) => {}
        Ok(CheckResult::Undetermined(QueryReason::Cancelled)) | Err(QueryReason::Cancelled) => {
            boundary(
                result,
                pc,
                FrontierReason::Relations(QueryReason::Cancelled),
            );
            return None;
        }
        Err(reason) => boundary(result, pc, FrontierReason::Relations(reason)),
        Ok(CheckResult::Undetermined(reason)) => result
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
