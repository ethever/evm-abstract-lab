//! Logical allocation arithmetic. Symbolic ranges arrive constrained by transfer.

use super::{RangeError, constant, expr, limits, numeric, zero};
use crate::domain::{AbstractValue, Domain, symbolic::ExprId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

/// Check the actual retained terms, including numeric-only operands whose
/// bounded source expression was synthesized by the caller.
pub(super) fn check_allocation(
    allocation: &(ExprId, ExprId),
    domain: Domain,
) -> Result<(), RangeError> {
    for expression in [&allocation.0, &allocation.1] {
        super::check_value(
            &AbstractValue::top().with_expression(expression.clone()),
            limits(domain),
        )?;
    }
    Ok(())
}

/// Bind numeric-only operands once, then reuse the same term for allocation,
/// roots and length arithmetic. No scalar fact is inferred from a path guard.
pub(super) fn normalized(value: &AbstractValue) -> Result<AbstractValue, RangeError> {
    Ok(value.clone().with_expression(expr(value)?))
}

pub(super) fn validate_target(
    offset: &AbstractValue,
    size: &AbstractValue,
    memory: bool,
) -> Result<(), RangeError> {
    if size.singleton() == Some(U256::ZERO) {
        return Ok(());
    }
    // A concrete extent needs a U256 end, never a host index. Symbolic success
    // is constrained at transfer; expanded() does not invent numeric guard facts.
    if let (Some(offset), Some(size)) = (offset.singleton(), size.singleton())
        && checked_end(offset, size, memory).is_none()
    {
        return Err(RangeError::OffsetTooLarge(offset));
    }
    Ok(())
}
fn op(opcode: u8, args: &[ExprId], domain: Domain) -> Result<ExprId, RangeError> {
    Ok(ExprId::operation(opcode, args, limits(domain))?)
}
fn choose(
    condition: ExprId,
    yes: ExprId,
    no: ExprId,
    domain: Domain,
) -> Result<ExprId, RangeError> {
    Ok(ExprId::ite(condition, yes, no, limits(domain))?)
}

fn checked_end(offset: U256, size: U256, memory: bool) -> Option<U256> {
    let end = offset.checked_add(size)?;
    if memory {
        end.checked_add(U256::from(31))
            .map(|v| v / U256::from(32) * U256::from(32))
    } else {
        Some(end)
    }
}

pub(super) fn expanded(
    old: &AbstractValue,
    offset: &AbstractValue,
    size: &AbstractValue,
    _max_bytes: usize,
    memory: bool,
    domain: Domain,
) -> Result<AbstractValue, RangeError> {
    if size.singleton() == Some(U256::ZERO) {
        return Ok(old.clone());
    }
    // The caller retains the allocation operands even when the length is
    // unchanged. A numeric shortcut must not bypass their policy or cancel.
    for value in [old, offset, size] {
        super::check_value(value, limits(domain))?;
    }
    if let (Some(at), Some(size)) = (offset.singleton(), size.singleton()) {
        let end = checked_end(at, size, memory).ok_or(RangeError::OffsetTooLarge(at))?;
        if end <= old.interval().unsigned_bounds().0 {
            return Ok(old.clone());
        }
        if let Some(old) = old.singleton() {
            return Ok(AbstractValue::constant(old.max(end)));
        }
    }
    let numerical = numeric::numeric_domain(domain);
    let mut finite = None;
    if let (Some(old), Some(offsets), Some(sizes)) =
        (old.constants(), offset.constants(), size.constants())
    {
        for old in old {
            for at in offsets {
                for size in sizes {
                    let candidate = if *size == U256::ZERO {
                        Some(*old)
                    } else {
                        at.checked_add(*size)
                            .and_then(|end| {
                                if memory {
                                    end.checked_add(U256::from(31))
                                        .map(|v| v / U256::from(32) * U256::from(32))
                                } else {
                                    Some(end)
                                }
                            })
                            .map(|end| end.max(*old))
                    };
                    if let Some(v) = candidate {
                        let v = AbstractValue::constant(v);
                        finite =
                            Some(finite.map_or_else(|| v.clone(), |old| numerical.join(&old, &v)));
                    } else {
                        finite = Some(AbstractValue::top());
                    }
                }
            }
        }
    }
    // A public wrapper call may lack scalar proof of the transfer guard.
    // Use operand bounds rather than importing an inactive relational guard.
    let endpoint_upper = checked_end(
        offset.interval().unsigned_bounds().1,
        size.interval().unsigned_bounds().1,
        memory,
    );
    let upper = old
        .interval()
        .unsigned_bounds()
        .1
        .max(endpoint_upper.unwrap_or(U256::MAX));
    let numeric = finite
        .filter(|value| !value.numeric().is_top())
        .unwrap_or_else(|| {
            AbstractValue::unsigned_range(old.interval().unsigned_bounds().0, upper)
                .unwrap_or_else(AbstractValue::top)
        });
    if let Some(value) = numeric.singleton() {
        return Ok(AbstractValue::constant(value));
    }
    // Prove the numeric result before constructing a duplicated max tree.
    // These bounds concern mathematical, nonwrapping endpoints. An unknown
    // upper endpoint cannot justify selecting a branch of a wrapped sum.
    if endpoint_upper.is_some_and(|end| end <= old.interval().unsigned_bounds().0) {
        return Ok(old.clone());
    }
    if endpoint_upper.is_none() {
        // A wrapped ADD/round-up is not an exact MSIZE expression. Retain one
        // bounded source for this allocation's coarse length; ByteArray stores
        // it, and repeated expansion of the same allocation reuses it.
        return normalized(&numeric);
    }
    let endpoint_lower = checked_end(
        offset.interval().unsigned_bounds().0,
        size.interval().unsigned_bounds().0,
        memory,
    );
    let size_expr = expr(size)?;
    let old_expr = expr(old)?;
    let end = op(opcode::ADD, &[expr(offset)?, size_expr.clone()], domain)?;
    let end = if memory {
        op(
            opcode::AND,
            &[
                op(
                    opcode::ADD,
                    &[end, ExprId::constant(U256::from(31))],
                    domain,
                )?,
                ExprId::constant(!U256::from(31)),
            ],
            domain,
        )?
    } else {
        end
    };
    let max = if end == old_expr {
        old_expr.clone()
    } else if endpoint_upper.is_some()
        && endpoint_lower.is_some_and(|end| old.interval().unsigned_bounds().1 <= end)
    {
        end
    } else {
        choose(
            op(opcode::GT, &[end.clone(), old_expr.clone()], domain)?,
            end,
            old_expr.clone(),
            domain,
        )?
    };
    let expression = if size.may_be_zero() {
        choose(
            op(
                opcode::EQ,
                &[size_expr, ExprId::constant(U256::ZERO)],
                domain,
            )?,
            old_expr,
            max,
            domain,
        )?
    } else {
        max
    };
    Ok(numeric::attach(numeric, expression))
}

pub(super) fn minimum(
    left: &AbstractValue,
    right: &AbstractValue,
    domain: Domain,
) -> Result<AbstractValue, RangeError> {
    if left.singleton() == Some(U256::ZERO) || right.singleton() == Some(U256::ZERO) {
        return Ok(zero());
    }
    let left_expr = expr(left)?;
    let right_expr = expr(right)?;
    let condition = op(opcode::LT, &[left_expr.clone(), right_expr.clone()], domain)?;
    let expression = choose(condition, left_expr, right_expr, domain)?;
    let numerical = numeric::numeric_domain(domain);
    let numeric = if let (Some(left), Some(right)) = (left.constants(), right.constants()) {
        let mut out = None;
        for a in left {
            for b in right {
                let v = AbstractValue::constant((*a).min(*b));
                out = Some(out.map_or_else(|| v.clone(), |old| numerical.join(&old, &v)));
            }
        }
        out.unwrap_or_else(|| constant(0))
    } else {
        let (alo, ahi) = left.interval().unsigned_bounds();
        let (blo, bhi) = right.interval().unsigned_bounds();
        AbstractValue::unsigned_range(alo.min(blo), ahi.min(bhi)).unwrap_or_else(AbstractValue::top)
    };
    Ok(numeric::attach(numeric, expression))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod logical_tests;
