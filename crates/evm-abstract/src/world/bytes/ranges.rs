//! Logical allocation arithmetic. Symbolic ranges arrive constrained by transfer.

use super::{RangeError, constant_with, expr, expr_without_query, limits, numeric, zero_with};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{EvmValue, symbolic::ExprId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

// Numeric certificates must remain independent of the retained expression.
// These exported views are cached; none invokes opaque N::singleton discovery.
pub(super) fn numeric_singleton<N: WordElement>(value: &EvmValue<N>) -> Option<U256> {
    value
        .constants()
        .filter(|values| values.len() == 1)
        .and_then(|values| values.first().copied())
        .or_else(|| value.known_bits().singleton())
        .or_else(|| {
            let (lower, upper) = value.interval().unsigned_bounds();
            (lower == upper).then_some(lower)
        })
}

// Shared independent proof for quotation and executable range shortcuts. A
// retained literal also proves a range, but cannot certify its own display.
pub(super) fn known_singleton<N: WordElement>(value: &EvmValue<N>) -> Option<U256> {
    value
        .expression()
        .and_then(ExprId::as_constant)
        .or_else(|| numeric_singleton(value))
}

// Existing allocation callers retain their discovery behavior. CALL's COPY fee
// owns independent range proofs and observations, without opaque query work.
#[derive(Clone, Copy)]
pub(super) enum Queries {
    Discover,
    Independent,
}

impl Queries {
    pub(super) fn singleton<N: WordElement>(self, value: &EvmValue<N>) -> Option<U256> {
        known_singleton(value).or_else(|| match self {
            Self::Discover => value.singleton(),
            Self::Independent => None,
        })
    }

    fn bounds<N: WordElement>(self, value: &EvmValue<N>) -> (U256, U256) {
        self.singleton(value)
            .map_or_else(|| value.interval().unsigned_bounds(), |word| (word, word))
    }

    pub(super) fn expression<N: WordElement>(
        self,
        value: &EvmValue<N>,
    ) -> Result<ExprId, RangeError> {
        match self {
            Self::Discover => expr(value),
            Self::Independent => expr_without_query(value),
        }
    }

    pub(super) fn normalized<N: WordElement>(
        self,
        value: &EvmValue<N>,
    ) -> Result<EvmValue<N>, RangeError> {
        Ok(value.clone().with_expression(self.expression(value)?))
    }
}

/// Check the actual retained terms, including numeric-only operands whose
/// bounded source expression was synthesized by the caller.
pub(super) fn check_allocation<D: EvmNumericAlgebra>(
    allocation: &(ExprId, ExprId),
    domain: AlgebraRef<'_, D>,
) -> Result<(), RangeError> {
    for expression in [&allocation.0, &allocation.1] {
        super::check_value(
            &EvmValue::<D::Element>::top().with_expression(expression.clone()),
            limits(domain),
        )?;
    }
    Ok(())
}

/// Bind numeric-only operands once, then reuse the same term for allocation,
/// roots and length arithmetic. No scalar fact is inferred from a path guard.
pub(super) fn normalized<N: WordElement>(value: &EvmValue<N>) -> Result<EvmValue<N>, RangeError> {
    Queries::Discover.normalized(value)
}

pub(super) fn normalized_without_query<N: WordElement>(
    value: &EvmValue<N>,
) -> Result<EvmValue<N>, RangeError> {
    Queries::Independent.normalized(value)
}

pub(super) fn validate_target<N: WordElement>(
    offset: &EvmValue<N>,
    size: &EvmValue<N>,
    memory: bool,
    queries: Queries,
) -> Result<(), RangeError> {
    if queries.singleton(size) == Some(U256::ZERO) {
        return Ok(());
    }
    // A concrete extent needs a U256 end, never a host index. Symbolic success
    // is constrained at transfer; expanded() does not invent numeric guard facts.
    if let (Some(offset), Some(size)) = (queries.singleton(offset), queries.singleton(size))
        && checked_end(offset, size, memory).is_none()
    {
        return Err(RangeError::OffsetTooLarge(offset));
    }
    Ok(())
}
fn op<D: EvmNumericAlgebra>(
    opcode: u8,
    args: &[ExprId],
    domain: AlgebraRef<'_, D>,
) -> Result<ExprId, RangeError> {
    Ok(ExprId::operation(opcode, args, limits(domain))?)
}
fn choose<D: EvmNumericAlgebra>(
    condition: ExprId,
    yes: ExprId,
    no: ExprId,
    domain: AlgebraRef<'_, D>,
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

pub(super) fn expanded_with_queries<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    old: &EvmValue<N>,
    offset: &EvmValue<N>,
    size: &EvmValue<N>,
    _max_bytes: usize,
    memory: bool,
    domain: AlgebraRef<'_, D>,
    queries: Queries,
) -> Result<EvmValue<N>, RangeError> {
    if queries.singleton(size) == Some(U256::ZERO) {
        return Ok(old.clone());
    }
    // The caller retains the allocation operands even when the length is
    // unchanged. A numeric shortcut must not bypass their policy or cancel.
    for value in [old, offset, size] {
        super::check_value(value, limits(domain))?;
    }
    if let (Some(at), Some(size)) = (queries.singleton(offset), queries.singleton(size)) {
        let end = checked_end(at, size, memory).ok_or(RangeError::OffsetTooLarge(at))?;
        if end <= queries.bounds(old).0 {
            return Ok(old.clone());
        }
        if let Some(old) = queries.singleton(old) {
            return Ok(EvmValue::<N>::constant(old.max(end)));
        }
    }
    let numerical = numeric::numeric_domain_with(domain);
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
                        let v = EvmValue::<N>::constant(v);
                        finite = Some(match finite {
                            None => v,
                            Some(old) => numerical.join(&old, &v),
                        });
                    } else {
                        finite = Some(EvmValue::<N>::top());
                    }
                }
            }
        }
    }
    // A public wrapper call may lack scalar proof of the transfer guard.
    // Use operand bounds rather than importing an inactive relational guard.
    let endpoint_upper = checked_end(queries.bounds(offset).1, queries.bounds(size).1, memory);
    let upper = old
        .interval()
        .unsigned_bounds()
        .1
        .max(endpoint_upper.unwrap_or(U256::MAX));
    let numeric = finite
        .filter(|value| !value.numeric().is_top())
        .unwrap_or_else(|| {
            EvmValue::<N>::unsigned_range(queries.bounds(old).0, upper)
                .unwrap_or_else(EvmValue::<N>::top)
        });
    if let Some(value) = queries.singleton(&numeric) {
        return Ok(EvmValue::<N>::constant(value));
    }
    // Prove the numeric result before constructing a duplicated max tree.
    // These bounds concern mathematical, nonwrapping endpoints. An unknown
    // upper endpoint cannot justify selecting a branch of a wrapped sum.
    if endpoint_upper.is_some_and(|end| end <= queries.bounds(old).0) {
        return Ok(old.clone());
    }
    if endpoint_upper.is_none() {
        // A wrapped ADD/round-up is not an exact MSIZE expression. Retain one
        // bounded source for this allocation's coarse length; ByteArray stores
        // it, and repeated expansion of the same allocation reuses it.
        let expression = queries.expression(&numeric)?;
        return Ok(numeric.with_expression(expression));
    }
    let endpoint_lower = checked_end(queries.bounds(offset).0, queries.bounds(size).0, memory);
    let size_expr = queries.expression(size)?;
    let old_expr = queries.expression(old)?;
    let end = op(
        opcode::ADD,
        &[queries.expression(offset)?, size_expr.clone()],
        domain,
    )?;
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
        && endpoint_lower.is_some_and(|end| queries.bounds(old).1 <= end)
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

pub(super) fn minimum<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &EvmValue<N>,
    right: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
) -> Result<EvmValue<N>, RangeError> {
    if known_singleton(left) == Some(U256::ZERO) || known_singleton(right) == Some(U256::ZERO) {
        // N::constant need not export a finite view. Carry this literal proof
        // into copy_from's zero guard instead of rediscovering it on the result.
        return Ok(zero_with().with_expression(ExprId::constant(U256::ZERO)));
    }
    let left_expr = expr_without_query(left)?;
    let right_expr = expr_without_query(right)?;
    let condition = op(opcode::LT, &[left_expr.clone(), right_expr.clone()], domain)?;
    let expression = choose(condition, left_expr, right_expr, domain)?;
    let numerical = numeric::numeric_domain_with(domain);
    let numeric = if let (Some(left), Some(right)) = (left.constants(), right.constants()) {
        let mut out = None;
        for a in left {
            for b in right {
                let v = EvmValue::<N>::constant((*a).min(*b));
                out = Some(match out {
                    None => v,
                    Some(old) => numerical.join(&old, &v),
                });
            }
        }
        out.unwrap_or_else(|| constant_with(0))
    } else {
        let (alo, ahi) = Queries::Independent.bounds(left);
        let (blo, bhi) = Queries::Independent.bounds(right);
        EvmValue::<N>::unsigned_range(alo.min(blo), ahi.min(bhi)).unwrap_or_else(EvmValue::<N>::top)
    };
    // Retain both the computed N and its folded literal certificate so nested
    // independent COPY can use the exact extent without rediscovering N.
    Ok(numeric.with_expression(expression))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod logical_tests;

#[cfg(test)]
fn expanded<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    old: &EvmValue<N>,
    offset: &EvmValue<N>,
    size: &EvmValue<N>,
    max_bytes: usize,
    memory: bool,
    domain: AlgebraRef<'_, D>,
) -> Result<EvmValue<N>, RangeError> {
    expanded_with_queries(
        old,
        offset,
        size,
        max_bytes,
        memory,
        domain,
        Queries::Discover,
    )
}
