//! Word predicates for mathematical byte ranges, before memory effects execute.
//!
//! Subtraction is guarded by the offset bound. This expresses a checked sum
//! without allowing EVM's modular arithmetic to wrap an invalid range to zero.

use crate::domain::{EvmNumericAlgebra, WordElement};

use crate::domain::{AlgebraRef, EvmValue};
use revm_bytecode::opcode;
use std::sync::Arc;

use super::{EvmExecution, EvmExecutionFrontier, TransferContext, finish};
use crate::{
    analysis::{
        EvmMachinePayload, EvmRangeRequest, EvmRangeWitness, FrontierReason, OutcomeKind, RangeKind,
    },
    domain::relational::{CheckResult, QueryReason},
    world::EvmByteArray,
};

mod guard;
mod logical;
mod retention;

use retention::{Failure, GuardRetention};

struct Partition<N: WordElement> {
    allowed: Option<EvmMachinePayload<N>>,
    excluded: Option<EvmMachinePayload<N>>,
}

struct Constrained<N: WordElement> {
    payload: Option<EvmMachinePayload<N>>,
    precision_loss: Option<QueryReason>,
}

impl<N: WordElement> Constrained<N> {
    fn exact(payload: Option<EvmMachinePayload<N>>) -> Self {
        Self {
            payload,
            precision_loss: None,
        }
    }
}

/// Admit a logical extent and a conservative gas-failure alternative before
/// this instruction changes memory. Host extraction limits are unrelated.
pub(super) fn memory<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    if !logical::failure(result, context, pc) {
        return false;
    }
    allow_memory(result, offset, size, context, pc)
}

/// MCOPY checks both endpoints against the captured, pre-instruction state.
/// One unguarded gas failure covers either endpoint without duplicate unwinds.
pub(super) fn memory_copy<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    source: &EvmValue<D::Element>,
    destination: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    logical::failure(result, context, pc)
        && allow_memory(result, source, size, context, pc)
        && allow_memory(result, destination, size, context, pc)
}

// Four-limb Word256: metadata covers cached root checks and one scalar quote;
// bounds cover two ordered comparisons and the short-circuited subtraction.
const LITERAL_MEMORY_METADATA_WORK: usize = 32 * 4;
const LITERAL_MEMORY_BOUNDS_WORK: usize = 32 * 4;

/// Quote the original singleton query from metadata only, after paying for this
/// quote. No candidate, set iteration, expression traversal or operand copy.
fn literal_memory_singleton_work<N: WordElement>(value: &EvmValue<N>) -> usize {
    let limbs = 4;
    let finite = value.finite_constants().cardinality().map_or(0, |len| {
        // A nonleaf root has >= 2 children; other internal nodes have >= 6
        // (hence >= 4). For h levels, len >= 2 * 4^(h-1) - 1. Use ceil(len/2)
        // without overflowing len+1; its base-4 log bounds the height. Below
        // eleven keys the tree must be a leaf. len=1 visits one level/one key.
        let levels = if len < 11 {
            1
        } else {
            1 + (len / 2 + len % 2).ilog2() as usize / 2
        };
        // NumericValue::singleton uses first() only at len=1, then contains()
        // validates the candidate, including when another component supplied it.
        let first = if len == 1 { 8 * limbs } else { 0 };
        let contains = levels * (len.min(11) * (6 * limbs + 8) + 8 * limbs);
        first + contains
    });
    // Top returns Some((1, 0)); Exact returns None. Only Modulo executes `%`.
    let modulo = !value.congruence().is_top() && value.congruence().modulus_residue().is_some();
    // 128 actions/limb cover all other singleton selection/validation, copied
    // scalar words and the zero test. Each tree comparison costs <= 6/limb + 8,
    // plus 8/limb per node. Only Modulo needs the pinned ruint 256*limbs^2
    // remainder bound (normalization, reciprocal, quotient/carry corrections).
    128 * limbs + finite + usize::from(modulo) * 256 * limbs * limbs
}

/// Inspect only bounded scalar facts; retained graphs and payloads stay shared.
/// Inherited refusals and invalid operand roots keep their original guard path.
fn literal_memory_range<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> Result<Option<bool>, FrontierReason> {
    if context.budget.exhausted() || !context.budget.charge(LITERAL_MEMORY_METADATA_WORK) {
        return Err(FrontierReason::Work);
    }
    let limits = context.config.analysis.relations;
    if [offset, size].into_iter().any(|value| {
        value.symbolic_limit_reached()
            || value.expression().is_some_and(|expression| {
                expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth
            })
    }) {
        return Ok(None);
    }
    if !context.budget.charge(literal_memory_singleton_work(size)) {
        return Err(FrontierReason::Work);
    }
    let Some(size) = size.singleton() else {
        return Ok(None);
    };
    if size == crate::U256::ZERO {
        return Ok(Some(true));
    }
    // A proven empty access never quotes or queries unrelated offset facts.
    // Refusal in a later phase retains every earlier successful reservation.
    if !context.budget.charge(LITERAL_MEMORY_METADATA_WORK) {
        return Err(FrontierReason::Work);
    }
    if !context.budget.charge(literal_memory_singleton_work(offset)) {
        return Err(FrontierReason::Work);
    }
    let Some(offset) = offset.singleton() else {
        return Ok(None);
    };
    if !context.budget.charge(LITERAL_MEMORY_BOUNDS_WORK) {
        return Err(FrontierReason::Work);
    }
    Ok(Some(
        offset <= logical::MAX_END && size <= logical::MAX_END - offset,
    ))
}

fn allow_memory<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    match literal_memory_range(offset, size, context) {
        Ok(Some(allowed)) => return allowed,
        Ok(None) => {}
        Err(reason) => {
            super::boundary(result, pc, reason);
            return false;
        }
    }
    let guard::PreparedGuard {
        request,
        condition,
        retention,
    } = match guard::build(offset, size, context) {
        Ok(guard) => guard,
        Err(_) => {
            super::boundary(result, pc, FrontierReason::Work);
            return false;
        }
    };
    if !condition.may_be_zero() {
        return true;
    }
    let Some(partition) = partition(result, &request, &condition, Some(&retention), context, pc)
    else {
        return false;
    };
    // The excluded nonempty domain is universally unaffordable under the
    // pinned fork costs; logical::failure already covers it and all affordable
    // or potentially unaffordable representable accesses. No policy frontier.
    let Some(payload) = partition.allowed else {
        return false;
    };
    result.payload = payload;
    true
}

/// Returndata source overflow is an execution failure, independently of the
/// destination-memory analysis policy. Empty copies still check the source.
pub(super) fn returndata<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    length: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    let request = EvmRangeRequest {
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
    let Some(partition) = partition(result, &request, &condition, None, context, pc) else {
        return false;
    };
    if let Some(payload) = partition.excluded {
        finish(
            result,
            payload,
            OutcomeKind::Failure,
            EvmByteArray::<D::Element>::empty(),
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

fn partition<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    request: &EvmRangeRequest<D::Element>,
    condition: &EvmValue<D::Element>,
    retention: Option<&GuardRetention>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> Option<Partition<D::Element>> {
    let cost = result
        .payload
        .work_size()
        .saturating_mul(3)
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
                        .map_err(|reason| Failure::plain(FrontierReason::Relations(reason)))?;
                }
            }
        }
        let allowed = constrained(&input, condition, true, retention, context)?;
        let excluded = constrained(&input, condition, false, retention, context)?;
        for reason in [allowed.precision_loss, excluded.precision_loss]
            .into_iter()
            .flatten()
        {
            result.diagnostics.push((
                pc,
                crate::analysis::DiagnosticKind::RelationalPrecisionLost(reason),
            ));
        }
        Ok::<_, Failure>(Partition {
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

fn constrained<D: EvmNumericAlgebra>(
    input: &EvmMachinePayload<D::Element>,
    condition: &EvmValue<D::Element>,
    nonzero: bool,
    retention: Option<&GuardRetention>,
    context: &mut TransferContext<'_, D>,
) -> Result<Constrained<D::Element>, Failure> {
    if !(if nonzero {
        condition.may_be_nonzero()
    } else {
        condition.may_be_zero()
    }) {
        return Ok(Constrained::<D::Element>::exact(None));
    }
    let mut payload = input.clone();
    if condition.singleton().is_some() {
        return Ok(Constrained::<D::Element>::exact(Some(payload)));
    }
    let expression = condition
        .symbolic_expression()
        .ok_or_else(|| Failure::missing_expression(retention))?;
    let limits = context.config.analysis.relations;
    let cost = payload
        .relations
        .work_size()
        .saturating_add(expression.work_size());
    if !context.budget.charge(cost) {
        return Err(Failure::plain(FrontierReason::Work));
    }
    let answer = payload.relations.assume_checked_budgeted_with(
        &expression,
        nonzero,
        &limits,
        context.budget,
        context.domain,
    );
    if context.budget.exhausted() {
        return Err(Failure::plain(FrontierReason::Work));
    }
    match answer {
        Ok(CheckResult::ProvenImpossible) => Ok(Constrained::<D::Element>::exact(None)),
        Ok(CheckResult::MayBeFeasible) => Ok(Constrained::<D::Element>::exact(Some(payload))),
        Err(reason) | Ok(CheckResult::Undetermined(reason @ QueryReason::Cancelled)) => {
            Err(Failure::plain(FrontierReason::Relations(reason)))
        }
        // These failures occur after retaining the assumption. The two guarded
        // domains still cover the input without treating UNKNOWN as UNSAT.
        Ok(CheckResult::Undetermined(reason)) => Ok(Constrained {
            payload: Some(payload),
            precision_loss: Some(reason),
        }),
    }
}

fn pending<N: WordElement>(
    result: &mut EvmExecution<N>,
    pc: usize,
    failure: Failure,
    request: EvmRangeRequest<N>,
    condition: EvmValue<N>,
    nonzero: Option<bool>,
    payload: EvmMachinePayload<N>,
) {
    let index = result.frontiers.len();
    result.frontiers.push(EvmExecutionFrontier {
        pc,
        reason: super::boundary_reason(failure.reason),
        target: Some(payload.key()),
        payload: None,
        range: Some(EvmRangeWitness {
            request,
            condition,
            nonzero,
            payload: Arc::new(payload),
        }),
    });
    if let Some(cause) = failure.retention {
        result.retention_failures.push((index, cause));
    }
}

/// A nonempty access must have a word-rounded U256 end. Empty accesses never
/// expand memory, including offset MAX. The subtraction stays guarded.
#[cfg(test)]
pub(super) fn memory_condition<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue {
    let limit = EvmValue::<D::Element>::constant(logical::MAX_END);
    let empty = domain.apply(opcode::ISZERO, std::slice::from_ref(size));
    let fits = sequence_condition(offset, size, &limit, domain);
    domain.apply(opcode::OR, &[empty, fits])
}

/// A returndata read must fit even for size zero: an offset beyond the end is
/// exceptional. The predicate also rejects an overflowing offset + size.
pub(super) fn sequence_condition<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    length: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue {
    let offset_fits = at_most(offset, length, domain);
    let remaining = domain.apply(opcode::SUB, &[length.clone(), offset.clone()]);
    let size_fits = at_most(size, &remaining, domain);
    domain.apply(opcode::AND, &[offset_fits, size_fits])
}

fn at_most<D: EvmNumericAlgebra>(
    left: &EvmValue<D::Element>,
    right: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue {
    let greater = domain.apply(opcode::GT, &[left.clone(), right.clone()]);
    domain.apply(opcode::ISZERO, &[greater])
}

#[cfg(test)]
mod tests;
