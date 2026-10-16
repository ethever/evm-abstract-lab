//! Fixed demands may use complete facts from their own immutable snapshot.

use super::{conditional::Context, numeric};
use crate::domain::{
    Domain,
    symbolic::{ByteStateKind, ByteView, ExprError, ExprId, ExprLimits},
};
use alloy_primitives::U256;

/// Resolve a complete word demand without constructing wrapping ADD terms.
/// All bounds belong to the caller's current guard environment. Returning None
/// preserves the view for bytewise evaluation of partial or padded windows.
pub(super) fn view_word_offset(
    view: &ByteView,
    at: &ExprId,
    context: &Context<'_>,
    domain: Domain,
) -> Option<ExprId> {
    let domain = numeric::numeric_domain(domain);
    let relative = numeric::scalar_in(at, context, domain);
    let offset = numeric::scalar_in(&view.offset, context, domain);
    let length = numeric::scalar_in(&view.length, context, domain);
    let last = relative
        .interval()
        .unsigned_bounds()
        .1
        .checked_add(U256::from(31))?;
    if last >= length.interval().unsigned_bounds().0 {
        return None;
    }
    offset.interval().unsigned_bounds().1.checked_add(last)?;
    if relative.singleton() == Some(U256::ZERO) {
        Some(view.offset.clone())
    } else if offset.singleton() == Some(U256::ZERO) {
        Some(at.clone())
    } else {
        Some(ExprId::constant(
            offset.singleton()?.checked_add(relative.singleton()?)?,
        ))
    }
}

/// Both the destination copy and the captured source view must cover all 32
/// bytes. In particular, a short CALL output never replaces its old suffix.
pub(super) fn copy_word_offset(
    destination: &ExprId,
    source: &ByteView,
    length: &ExprId,
    at: &ExprId,
    context: &Context<'_>,
    domain: Domain,
) -> Option<ExprId> {
    let domain = numeric::numeric_domain(domain);
    let offset = numeric::scalar_in(at, context, domain);
    offset
        .interval()
        .unsigned_bounds()
        .1
        .checked_add(U256::from(31))?;
    let relative = if at == destination {
        U256::ZERO
    } else {
        offset
            .singleton()?
            .checked_sub(numeric::scalar_in(destination, context, domain).singleton()?)?
    };
    if relative.checked_add(U256::from(31))?
        >= numeric::scalar_in(length, context, domain)
            .interval()
            .unsigned_bounds()
            .0
    {
        return None;
    }
    view_word_offset(source, &ExprId::constant(relative), context, domain)
}

pub(super) fn same_offset(
    left: &ExprId,
    right: &ExprId,
    context: &Context<'_>,
    domain: Domain,
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
pub(super) fn word_expression(
    mut view: ByteView,
    at: ExprId,
    context: &Context<'_>,
    domain: Domain,
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
