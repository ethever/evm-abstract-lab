//! Fixed demands use guarantees from their own immutable snapshot.
//! Pure and paid reads share the same complete-word forwarding plans.

use super::{conditional::Context, numeric};
use crate::domain::{
    AlgebraRef, EvmNumericAlgebra, WordElement,
    symbolic::{ByteStateKind, ByteView, ExprError, ExprId, ExprLimits},
};
use crate::resource::{CloneError, SummaryError};
use alloy_primitives::U256;
use numeric::payment::{ReadPayment, ReadResult};

#[cfg(test)]
pub(super) fn nonwrapping_word<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> bool {
    nonwrapping_word_paid(at, context, domain, &mut ReadPayment::unpaid()).unwrap_or(false)
}
pub(super) fn nonwrapping_word_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<bool> {
    let value = numeric::scalar_in_paid(at, context, domain, payer)?;
    let last = U256::MAX - U256::from(31);
    if payer.is_paid() {
        payer.reserve(value.numeric().work_size())?;
    }
    if value.interval().unsigned_bounds().1 <= last {
        return Ok(true);
    }
    let singleton = match at.as_constant() {
        Some(n) => Some(n),
        None => {
            if payer.is_paid() {
                payer.singleton(&value)?
            } else {
                context.prove_singleton(&value)
            }
        }
    };
    payer.context(context)?;
    Ok(singleton.is_some_and(|offset| offset <= last))
}
pub(super) fn view_word_offset_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    view: &ByteView,
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<Option<ExprId>> {
    let domain = numeric::numeric_domain(domain);
    let relative = numeric::scalar_in_paid(at, context, domain, payer)?;
    let offset = numeric::scalar_in_paid(&view.offset, context, domain, payer)?;
    let length = numeric::scalar_in_paid(&view.length, context, domain, payer)?;
    payer.reserve(relative.numeric().work_size())?;
    let upper = at
        .as_constant()
        .unwrap_or_else(|| relative.interval().unsigned_bounds().1);
    let Some(last) = upper.checked_add(U256::from(31)) else {
        return Ok(None);
    };
    let lower = match view.length.as_constant() {
        Some(n) => n,
        None => payer.bounds(&length)?.0,
    };
    if last >= lower {
        return Ok(None);
    }
    let upper = match view.offset.as_constant() {
        Some(n) => n,
        None => payer.bounds(&offset)?.1,
    };
    if upper.checked_add(last).is_none() {
        return Ok(None);
    }
    let relative_single = match at.as_constant() {
        Some(n) => Some(n),
        None => payer.singleton(&relative)?,
    };
    if relative_single == Some(U256::ZERO) {
        return Ok(Some(view.offset.clone()));
    }
    let offset_single = match view.offset.as_constant() {
        Some(n) => Some(n),
        None => payer.singleton(&offset)?,
    };
    if offset_single == Some(U256::ZERO) {
        return Ok(Some(at.clone()));
    }
    // Preserve the original independent discoveries when neither zero shortcut
    // applies; each query on an opaque native value owns its actual tariff.
    let a = match view.offset.as_constant() {
        Some(n) => Some(n),
        None => payer.singleton(&offset)?,
    };
    let b = match at.as_constant() {
        Some(n) => Some(n),
        None => payer.singleton(&relative)?,
    };
    Ok(a.zip(b)
        .and_then(|(a, b)| a.checked_add(b))
        .map(ExprId::constant))
}
pub(super) fn copy_word_offset_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    destination: &ExprId,
    source: &ByteView,
    length: &ExprId,
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<Option<ExprId>> {
    let domain = numeric::numeric_domain(domain);
    let offset = numeric::scalar_in_paid(at, context, domain, payer)?;
    payer.reserve(
        offset
            .numeric()
            .work_size()
            .saturating_add(at.comparison_work(destination)),
    )?;
    let upper = at
        .as_constant()
        .unwrap_or_else(|| offset.interval().unsigned_bounds().1);
    if upper.checked_add(U256::from(31)).is_none() {
        return Ok(None);
    }
    let relative = if at == destination {
        U256::ZERO
    } else {
        let Some(offset) = payer.singleton(&offset)? else {
            return Ok(None);
        };
        let destination = numeric::scalar_in_paid(destination, context, domain, payer)?;
        let Some(destination) = payer.singleton(&destination)? else {
            return Ok(None);
        };
        let Some(relative) = offset.checked_sub(destination) else {
            return Ok(None);
        };
        relative
    };
    let Some(end) = relative.checked_add(U256::from(31)) else {
        return Ok(None);
    };
    let value = numeric::scalar_in_paid(length, context, domain, payer)?;
    payer.reserve(value.numeric().work_size())?;
    let lower = length
        .as_constant()
        .unwrap_or_else(|| value.interval().unsigned_bounds().0);
    if end >= lower {
        return Ok(None);
    }
    view_word_offset_paid(source, &ExprId::constant(relative), context, domain, payer)
}
pub(super) fn same_offset_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &ExprId,
    right: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<bool> {
    payer.reserve(left.comparison_work(right))?;
    if left == right {
        return Ok(true);
    }
    let left = numeric::scalar_in_paid(left, context, numeric::numeric_domain(domain), payer)?;
    let a = payer.singleton(&left)?;
    let right = numeric::scalar_in_paid(right, context, numeric::numeric_domain(domain), payer)?;
    let b = payer.singleton(&right)?;
    Ok(a.zip(b).is_some_and(|(a, b)| a == b))
}
pub(super) fn word_expression<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    view: ByteView,
    at: ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    limits: ExprLimits,
) -> Result<ExprId, ExprError> {
    word_expression_paid(
        view,
        at,
        context,
        domain,
        limits,
        &mut ReadPayment::unpaid(),
    )
    .map_err(|error| match error {
        SummaryError::Source(super::RangeError::Expression(error)) => error,
        SummaryError::Budget(CloneError::Cancelled) => ExprError::Cancelled,
        _ => ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX,
        },
    })
}
pub(super) fn word_expression_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    mut view: ByteView,
    at: ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    limits: ExprLimits,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<ExprId> {
    payer.context(context)?;
    payer.reserve(
        view.nodes()
            .saturating_add(at.nodes())
            .saturating_add(1)
            .saturating_mul(32),
    )?;
    view = view.canonicalize();
    crate::domain::symbolic::state_check(
        view.nodes().saturating_add(at.nodes()).saturating_add(1),
        view.depth().max(at.depth()).saturating_add(1),
        limits,
    )
    .map_err(|e| SummaryError::Source(e.into()))?;
    let offset = view_word_offset_paid(&view, &at, context, domain, payer)?;
    if let Some(start) = offset.as_ref().and_then(ExprId::as_constant) {
        // This shared proof has already established nonwrapping start+31.
        let cursor = numeric::demanded_state_paid(
            &view.state,
            &ExprId::constant(start),
            32,
            context,
            domain,
            payer,
        )?;
        crate::domain::symbolic::state_check(cursor.nodes(), cursor.depth(), limits)
            .map_err(|e| SummaryError::Source(e.into()))?;
        view.state = cursor.clone();
    }
    if let Some(offset) = &offset {
        let forwarded = match view.state.kind() {
            ByteStateKind::Slice { source } => {
                view_word_offset_paid(source, offset, context, domain, payer)?
                    .map(|at| (source.state.clone(), at))
            }
            ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } => {
                copy_word_offset_paid(destination, source, length, offset, context, domain, payer)?
                    .map(|at| (source.state.clone(), at))
            }
            ByteStateKind::WriteWord {
                offset: written,
                value,
                ..
            } if same_offset_paid(written, offset, context, domain, payer)? => {
                crate::domain::symbolic::state_check(value.nodes(), value.depth(), limits)
                    .map_err(|e| SummaryError::Source(e.into()))?;
                return Ok(value.clone());
            }
            _ => None,
        };
        if let Some((state, offset)) = forwarded {
            return word_expression_paid(
                ByteView::new(state, offset, ExprId::constant(U256::from(32))),
                ExprId::constant(U256::ZERO),
                context,
                domain,
                limits,
                payer,
            );
        }
    }
    if (offset.is_some()
        || (view.offset.as_constant().is_some()
            && view.length.as_constant().is_some()
            && at.as_constant().is_some()))
        && let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = view.state.kind()
    {
        let then_context = context.branch_paid(condition, true, payer)?;
        let then_value = word_expression_paid(
            ByteView::new(then_state.clone(), view.offset.clone(), view.length.clone()),
            at.clone(),
            &then_context,
            domain,
            limits,
            payer,
        )?;
        let else_context = context.branch_paid(condition, false, payer)?;
        let else_value = word_expression_paid(
            ByteView::new(else_state.clone(), view.offset.clone(), view.length.clone()),
            at.clone(),
            &else_context,
            domain,
            limits,
            payer,
        )?;
        return ExprId::ite(condition.clone(), then_value, else_value, limits)
            .map_err(|e| SummaryError::Source(e.into()));
    }
    let payment = std::cell::RefCell::new(&mut *payer);
    let failure = std::cell::RefCell::new(None);
    let out = ExprId::word_read_with_write_bound(view, at, limits, &|destination| {
        if failure.borrow().is_some() {
            return None;
        }
        let mut payer = payment.borrow_mut();
        match context.write_lower_bound_paid(destination, domain, &mut payer) {
            Ok(bound) => bound,
            Err(error) => {
                *failure.borrow_mut() = Some(error);
                None
            }
        }
    });
    if let Some(error) = failure.into_inner() {
        return Err(error);
    }
    out.map_err(|e| SummaryError::Source(e.into()))
}
