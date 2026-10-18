//! Exact shared-branch reductions for complete byte and slot state handles.

use super::{ExprId, StateError, StateLimits};
use alloy_primitives::U256;

/// Inspect only the two immediate state kinds and collision-checked identities.
/// Each reduction removes at least one state ITE from the alternative tree;
/// callers repeat construction on its children, never scan the state graph.
/// Scalar guards retain the original independent Word256 nonzero semantics.
/// Existing join precharges cover expanded input metadata: each removed state
/// ITE pays for one bounded scalar ITE, retaining its guards without a new walk.
pub(in super::super) fn shared_branch<'a, S: Eq>(
    yes: &'a S,
    no: &'a S,
    selector: ExprId,
    yes_ite: Option<(&ExprId, &'a S, &'a S)>,
    no_ite: Option<(&ExprId, &'a S, &'a S)>,
    limits: StateLimits,
) -> Result<Option<(ExprId, &'a S, &'a S)>, StateError> {
    let (then_guard, else_guard, a, b) = match (yes_ite, no_ite) {
        // I(s,I(g,A,B),I(h,A,B)) = I(I(s,g,h),A,B).
        (Some((g, a, b)), Some((h, c, d))) if a == c && b == d => (g.clone(), h.clone(), a, b),
        // I(s,I(t,A,B),A) = I(I(s,t,1),A,B).
        (Some((t, a, b)), _) if no == a => (t.clone(), ExprId::constant(U256::from(1)), a, b),
        // I(s,I(t,A,B),B) = I(I(s,t,0),A,B).
        (Some((t, a, b)), _) if no == b => (t.clone(), ExprId::constant(U256::ZERO), a, b),
        // I(s,A,I(t,A,B)) = I(I(s,1,t),A,B).
        (_, Some((t, a, b))) if yes == a => (ExprId::constant(U256::from(1)), t.clone(), a, b),
        // I(s,B,I(t,A,B)) = I(I(s,0,t),A,B).
        (_, Some((t, a, b))) if yes == b => (ExprId::constant(U256::ZERO), t.clone(), a, b),
        _ => return Ok(None),
    };
    // The caller checked the original children first. This bounded constructor
    // checks the new scalar guard before any normalized state is published.
    let guard = ExprId::ite(selector, then_guard, else_guard, limits.into())?;
    Ok(Some((guard, a, b)))
}
