//! Fixed demands may use complete facts from their own immutable snapshot.

use super::{conditional::Context, numeric};
use crate::domain::AlgebraRef;
use crate::domain::symbolic::{ByteStateKind, ByteView, ExprError, ExprId, ExprLimits};
use crate::domain::{EvmNumericAlgebra, WordElement};
use alloy_primitives::U256;

/// The optional interval view may omit an exact custom singleton. Preserve the
/// old sufficient proof and fee first; only a new numeric proof pays extra.
pub(super) fn nonwrapping_word<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> bool {
    let value = numeric::scalar_in(at, context, domain);
    let last_start = U256::MAX - U256::from(31);
    value.interval().unsigned_bounds().1 <= last_start
        || at
            .as_constant()
            .or_else(|| context.prove_singleton(&value))
            .is_some_and(|offset| offset <= last_start)
}

/// Resolve a complete word demand without constructing wrapping ADD terms.
/// All bounds belong to the caller's current guard environment. Returning None
/// preserves the view for bytewise evaluation of partial or padded windows.
pub(super) fn view_word_offset<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    view: &ByteView,
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> Option<ExprId> {
    let domain = numeric::numeric_domain(domain);
    let relative = numeric::scalar_in(at, context, domain);
    let offset = numeric::scalar_in(&view.offset, context, domain);
    let length = numeric::scalar_in(&view.length, context, domain);
    let upper = at
        .as_constant()
        .unwrap_or_else(|| relative.interval().unsigned_bounds().1);
    let last = upper.checked_add(U256::from(31))?;
    let length_lower = view
        .length
        .as_constant()
        .unwrap_or_else(|| crate::world::unsigned_bounds(&length).0);
    if last >= length_lower {
        return None;
    }
    view.offset
        .as_constant()
        .unwrap_or_else(|| crate::world::unsigned_bounds(&offset).1)
        .checked_add(last)?;
    if at.as_constant().or_else(|| relative.singleton()) == Some(U256::ZERO) {
        Some(view.offset.clone())
    } else if view.offset.as_constant().or_else(|| offset.singleton()) == Some(U256::ZERO) {
        Some(at.clone())
    } else {
        Some(ExprId::constant(
            view.offset
                .as_constant()
                .or_else(|| offset.singleton())?
                .checked_add(at.as_constant().or_else(|| relative.singleton())?)?,
        ))
    }
}

/// Both the destination copy and the captured source view must cover all 32
/// bytes. In particular, a short CALL output never replaces its old suffix.
pub(super) fn copy_word_offset<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    destination: &ExprId,
    source: &ByteView,
    length: &ExprId,
    at: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> Option<ExprId> {
    let domain = numeric::numeric_domain(domain);
    let offset = numeric::scalar_in(at, context, domain);
    at.as_constant()
        .unwrap_or_else(|| offset.interval().unsigned_bounds().1)
        .checked_add(U256::from(31))?;
    let relative = if at == destination {
        U256::ZERO
    } else {
        offset
            .singleton()?
            .checked_sub(numeric::scalar_in(destination, context, domain).singleton()?)?
    };
    let end = relative.checked_add(U256::from(31))?;
    let length_value = numeric::scalar_in(length, context, domain);
    if end
        >= length
            .as_constant()
            .unwrap_or_else(|| length_value.interval().unsigned_bounds().0)
    {
        return None;
    }
    view_word_offset(source, &ExprId::constant(relative), context, domain)
}

pub(super) fn same_offset<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &ExprId,
    right: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> bool {
    left == right
        || numeric::scalar_in(left, context, numeric::numeric_domain(domain))
            .singleton()
            .zip(numeric::scalar_in(right, context, numeric::numeric_domain(domain)).singleton())
            .is_some_and(|(left, right)| left == right)
}

/// Normalize each whole-word alternative using only facts selected by that
/// snapshot's guard. The public constructor's global callback cannot express
/// different destination bounds in the two children of the same state ITE.
pub(super) fn word_expression<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    mut view: ByteView,
    at: ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    limits: ExprLimits,
) -> Result<ExprId, ExprError> {
    view = view.canonicalize();
    crate::domain::symbolic::state_check(
        view.nodes().saturating_add(at.nodes()).saturating_add(1),
        view.depth().max(at.depth()).saturating_add(1),
        limits,
    )?;
    let offset = view_word_offset(&view, &at, context, domain);
    if let Some(start) = offset.as_ref().and_then(ExprId::as_constant) {
        // The complete-window proof also established that start + 31 fits.
        let end = start + U256::from(31);
        let mut cursor = &view.state;
        loop {
            crate::domain::symbolic::state_check(cursor.nodes(), cursor.depth(), limits)?;
            let Some(previous) =
                cursor.prior_to_disjoint_write_with_bound(start, end, &|destination| {
                    context.write_lower_bound(destination, domain)
                })
            else {
                break;
            };
            cursor = previous;
        }
        view.state = cursor.clone();
    }
    if let Some(offset) = &offset {
        let forwarded = match view.state.kind() {
            ByteStateKind::Slice { source } => view_word_offset(source, offset, context, domain)
                .map(|at| (source.state.clone(), at)),
            ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } => copy_word_offset(destination, source, length, offset, context, domain)
                .map(|at| (source.state.clone(), at)),
            ByteStateKind::WriteWord {
                offset: written,
                value,
                ..
            } if same_offset(written, offset, context, domain) => {
                crate::domain::symbolic::state_check(value.nodes(), value.depth(), limits)?;
                return Ok(value.clone());
            }
            _ => None,
        };
        if let Some((state, offset)) = forwarded {
            return word_expression(
                ByteView::new(state, offset, ExprId::constant(U256::from(32))),
                ExprId::constant(U256::ZERO),
                context,
                domain,
                limits,
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
        let read = |state, nonzero| {
            let branch = ByteView::new(state, view.offset.clone(), view.length.clone());
            word_expression(
                branch,
                at.clone(),
                &context.branch(condition, nonzero),
                domain,
                limits,
            )
        };
        return ExprId::ite(
            condition.clone(),
            read(then_state.clone(), true)?,
            read(else_state.clone(), false)?,
            limits,
        );
    }
    ExprId::word_read_with_write_bound(view, at, limits, &|destination| {
        context.write_lower_bound(destination, domain)
    })
}
