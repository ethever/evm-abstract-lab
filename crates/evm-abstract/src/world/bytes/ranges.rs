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

pub(super) fn validate_size(size: &AbstractValue, cap: usize) -> Result<(), RangeError> {
    if size.constants().is_none() && size.expression().is_none() {
        return Err(RangeError::UnknownSize);
    }
    if let Some(size) = size.singleton()
        && size > U256::from(cap)
    {
        return Err(RangeError::TooLarge {
            requested: size,
            limit: cap,
        });
    }
    Ok(())
}
pub(super) fn validate_target(
    offset: &AbstractValue,
    size: &AbstractValue,
    cap: usize,
    memory: bool,
) -> Result<(), RangeError> {
    validate_size(size, cap)?;
    if size.singleton() == Some(U256::ZERO) {
        return Ok(());
    }
    // Nonenumerated symbolic operands retain their expression. The transfer
    // layer keeps both legal-policy and excluded branches with their conditions.
    if let (Some(offset), Some(size)) = (offset.singleton(), size.singleton()) {
        let start = usize::try_from(offset).map_err(|_| RangeError::OffsetTooLarge(offset))?;
        let count = usize::try_from(size).map_err(|_| RangeError::TooLarge {
            requested: size,
            limit: cap,
        })?;
        let end = start.checked_add(count).ok_or(RangeError::Overflow {
            offset: start,
            size: count,
        })?;
        let end = if memory {
            end.checked_add(31).ok_or(RangeError::Overflow {
                offset: start,
                size: count,
            })? / 32
                * 32
        } else {
            end
        };
        if end > cap {
            return Err(RangeError::TooLarge {
                requested: U256::from(end),
                limit: cap,
            });
        }
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
    cap: usize,
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
        let end = at.checked_add(size).ok_or(RangeError::TooLarge {
            requested: U256::MAX,
            limit: cap,
        })?;
        let end = if memory {
            end.checked_add(U256::from(31))
                .ok_or(RangeError::TooLarge {
                    requested: U256::MAX,
                    limit: cap,
                })?
                / U256::from(32)
                * U256::from(32)
        } else {
            end
        };
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
    // Use operand bounds rather than assuming cap is already a numeric fact.
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
    let numeric = finite.unwrap_or_else(|| {
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
    let condition = op(opcode::LT, &[expr(left)?, expr(right)?], domain)?;
    let expression = choose(condition, expr(left)?, expr(right)?, domain)?;
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
