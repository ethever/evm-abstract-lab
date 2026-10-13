//! Word predicates for mathematical byte ranges, before memory effects execute.
//!
//! Subtraction is guarded by the offset bound. This expresses a checked sum
//! without allowing EVM's modular arithmetic to wrap an invalid range to zero.

use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::sync::Arc;

use super::{Execution, ExecutionFrontier, TransferContext, finish};
use crate::{
    analysis::{
        FrontierReason, MachinePayload, OutcomeKind, RangeKind, RangeRequest, RangeWitness,
    },
    domain::relational::{CheckResult, QueryReason},
    world::ByteArray,
};

struct Partition {
    allowed: Option<MachinePayload>,
    excluded: Option<MachinePayload>,
}

struct Constrained {
    payload: Option<MachinePayload>,
    precision_loss: Option<QueryReason>,
}

impl Constrained {
    fn exact(payload: Option<MachinePayload>) -> Self {
        Self {
            payload,
            precision_loss: None,
        }
    }
}

/// Keep excluded inputs before memory changes. A solver UNKNOWN may retain
/// both guarded branches; inability to retain the guard stops this instruction.
pub(super) fn memory(
    result: &mut Execution,
    offset: &AbstractValue,
    size: &AbstractValue,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> bool {
    let request = RangeRequest {
        kind: RangeKind::MemoryPolicy,
        offset: super::relations::identify(
            offset.clone(),
            context.config.analysis.relations.enabled,
        ),
        size: super::relations::identify(size.clone(), context.config.analysis.relations.enabled),
        limit: super::constant(context.config.max_memory_bytes),
    };
    let condition = memory_condition(
        &request.offset,
        &request.size,
        context.config.max_memory_bytes,
        context.domain,
    );
    if !condition.may_be_zero() {
        return true;
    }
    let Some(partition) = partition(result, &request, &condition, context, pc) else {
        return false;
    };
    if let Some(payload) = partition.excluded {
        pending(
            result,
            pc,
            FrontierReason::Memory,
            request,
            condition,
            Some(false),
            payload,
        );
    }
    let Some(payload) = partition.allowed else {
        return false;
    };
    result.payload = payload;
    true
}

/// Returndata source overflow is an execution failure, independently of the
/// destination-memory analysis policy. Empty copies still check the source.
pub(super) fn returndata(
    result: &mut Execution,
    offset: &AbstractValue,
    size: &AbstractValue,
    length: &AbstractValue,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> bool {
    let request = RangeRequest {
        kind: RangeKind::ReturnData,
        offset: super::relations::identify(
            offset.clone(),
            context.config.analysis.relations.enabled,
        ),
        size: super::relations::identify(size.clone(), context.config.analysis.relations.enabled),
        limit: super::relations::identify(
            length.clone(),
            context.config.analysis.relations.enabled,
        ),
    };
    let condition = sequence_condition(
        &request.offset,
        &request.size,
        &request.limit,
        context.domain,
    );
    if !condition.may_be_zero() {
        return true;
    }
    let Some(partition) = partition(result, &request, &condition, context, pc) else {
        return false;
    };
    if let Some(payload) = partition.excluded {
        finish(
            result,
            payload,
            OutcomeKind::Failure,
            ByteArray::empty(),
            context,
            pc,
        );
    }
    let Some(payload) = partition.allowed else {
        return false;
    };
    result.payload = payload;
    true
}

fn partition(
    result: &mut Execution,
    request: &RangeRequest,
    condition: &AbstractValue,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> Option<Partition> {
    let cost = result
        .payload
        .work_size()
        .saturating_mul(2)
        .saturating_add(condition.work_size());
    if !context.budget.charge(cost) {
        super::boundary(result, pc, FrontierReason::Work);
        return None;
    }
    let mut input = result.payload.clone();
    let checked = (|| {
        if condition.may_be_zero() && condition.may_be_nonzero() {
            let limits = context.config.analysis.relations;
            for value in [&request.offset, &request.size, &request.limit] {
                if let Some(expression) = value.expression() {
                    input
                        .relations
                        .observe(expression, value.numeric(), &limits)
                        .map_err(FrontierReason::Relations)?;
                }
            }
        }
        let allowed = constrained(&input, condition, true, context)?;
        let excluded = constrained(&input, condition, false, context)?;
        for reason in [allowed.precision_loss, excluded.precision_loss]
            .into_iter()
            .flatten()
        {
            result.diagnostics.push((
                pc,
                crate::analysis::DiagnosticKind::RelationalPrecisionLost(reason),
            ));
        }
        Ok::<_, FrontierReason>(Partition {
            allowed: allowed.payload,
            excluded: excluded.payload,
        })
    })();
    match checked {
        Ok(partition) => Some(partition),
        Err(reason) => {
            pending(
                result,
                pc,
                reason,
                request.clone(),
                condition.clone(),
                None,
                input,
            );
            None
        }
    }
}

fn constrained(
    input: &MachinePayload,
    condition: &AbstractValue,
    nonzero: bool,
    context: &mut TransferContext<'_>,
) -> Result<Constrained, FrontierReason> {
    if !(if nonzero {
        condition.may_be_nonzero()
    } else {
        condition.may_be_zero()
    }) {
        return Ok(Constrained::exact(None));
    }
    let mut payload = input.clone();
    if condition.singleton().is_some() {
        return Ok(Constrained::exact(Some(payload)));
    }
    let expression = condition
        .symbolic_expression()
        .ok_or(FrontierReason::Relations(QueryReason::ExpressionLimit))?;
    let limits = context.config.analysis.relations;
    let cost = payload
        .relations
        .work_size()
        .saturating_add(expression.work_size())
        .saturating_add(limits.rlimit as usize);
    if !context.budget.charge(cost) {
        return Err(FrontierReason::Work);
    }
    match payload
        .relations
        .assume_checked(&expression, nonzero, &limits)
    {
        Ok(CheckResult::Unsat) => Ok(Constrained::exact(None)),
        Ok(CheckResult::Sat) => Ok(Constrained::exact(Some(payload))),
        Err(reason) | Ok(CheckResult::Unknown(reason @ QueryReason::Cancelled)) => {
            Err(FrontierReason::Relations(reason))
        }
        // These failures occur after retaining the assumption. The two guarded
        // domains still cover the input without treating UNKNOWN as UNSAT.
        Ok(CheckResult::Unknown(reason)) => Ok(Constrained {
            payload: Some(payload),
            precision_loss: Some(reason),
        }),
    }
}

fn pending(
    result: &mut Execution,
    pc: usize,
    reason: FrontierReason,
    request: RangeRequest,
    condition: AbstractValue,
    nonzero: Option<bool>,
    payload: MachinePayload,
) {
    result.frontiers.push(ExecutionFrontier {
        pc,
        reason: super::boundary_reason(reason),
        target: Some(payload.key()),
        payload: None,
        range: Some(RangeWitness {
            request,
            condition,
            nonzero,
            payload: Arc::new(payload),
        }),
    });
}

/// A nonempty access must fit the word-rounded logical memory policy. An empty
/// access never expands memory, even when its offset is not host representable.
pub(super) fn memory_condition(
    offset: &AbstractValue,
    size: &AbstractValue,
    maximum: usize,
    domain: Domain,
) -> AbstractValue {
    let limit = AbstractValue::constant(U256::from(maximum - maximum % 32));
    let empty = domain.apply(opcode::ISZERO, std::slice::from_ref(size));
    let fits = sequence_condition(offset, size, &limit, domain);
    domain.apply(opcode::OR, &[empty, fits])
}

/// A returndata read must fit even for size zero: an offset beyond the end is
/// exceptional. The predicate also rejects an overflowing offset + size.
pub(super) fn sequence_condition(
    offset: &AbstractValue,
    size: &AbstractValue,
    length: &AbstractValue,
    domain: Domain,
) -> AbstractValue {
    let offset_fits = at_most(offset, length, domain);
    let remaining = domain.apply(opcode::SUB, &[length.clone(), offset.clone()]);
    let size_fits = at_most(size, &remaining, domain);
    domain.apply(opcode::AND, &[offset_fits, size_fits])
}

fn at_most(left: &AbstractValue, right: &AbstractValue, domain: Domain) -> AbstractValue {
    let greater = domain.apply(opcode::GT, &[left.clone(), right.clone()]);
    domain.apply(opcode::ISZERO, &[greater])
}

#[cfg(test)]
mod tests;
