//! Fixed demands may use complete facts from their own immutable snapshot.

use super::conditional::Context;
use crate::domain::{
    Domain,
    symbolic::{ByteStateKind, ByteView, ExprError, ExprId, ExprLimits},
};
use alloy_primitives::U256;

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
    if let (Some(offset), Some(relative), Some(length)) = (
        view.offset.as_constant(),
        at.as_constant(),
        view.length.as_constant(),
    ) && relative
        .checked_add(U256::from(32))
        .is_some_and(|end| end <= length)
        && let Some(start) = offset.checked_add(relative)
    {
        let end = start.saturating_add(U256::from(31));
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
    if view.offset.as_constant().is_some()
        && view.length.as_constant().is_some()
        && at.as_constant().is_some()
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
