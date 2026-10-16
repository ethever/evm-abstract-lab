//! Operand guarantees include singleton sources, independently of numeric equality.

use super::check_value;
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{ExprError, ExprId, ExprLimits},
};
use std::collections::BTreeMap;

/// These are unconditional guarantees, so a missing entry on either path means
/// no guarantee. The immutable roots keep branch effects even when their local
/// operand bounds cannot be retained outside that branch.
pub(in crate::world) fn join_common(
    left: &BTreeMap<ExprId, AbstractValue>,
    right: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
) -> BTreeMap<ExprId, AbstractValue> {
    left.iter()
        .filter_map(|(expression, value)| {
            right
                .get(expression)
                .map(|other| (expression.clone(), domain.join(value, other)))
        })
        .collect()
}

/// Merge source alternatives, never infer input identity from an equal number.
/// The caller stages this map so a rejected metadata bound remains atomic.
pub(in crate::world) fn remember(
    facts: &mut BTreeMap<ExprId, AbstractValue>,
    expression: ExprId,
    value: &AbstractValue,
    domain: Domain,
    limits: ExprLimits,
) -> Result<(), ExprError> {
    check_value(value, limits)?;
    if expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth {
        return Err(ExprError::Limit {
            nodes: expression.nodes(),
            depth: expression.depth(),
        });
    }
    let mut clean = value.clone();
    clean.forget_identity();
    if let Some(constant) = expression.as_constant() {
        // One constant entry describes possible sources, not an occurrence of
        // a scoped input. Keep its exact numeric proof and remove must-aliases.
        clean = domain
            .join(&clean, &AbstractValue::constant(constant))
            .with_numeric(crate::domain::NumericValue::constant(constant));
    }
    if expression
        .as_constant()
        .is_none_or(|n| clean != AbstractValue::constant(n))
        || facts.contains_key(&expression)
    {
        insert(facts, expression, &clean, domain);
    }
    if facts.len() > limits.max_nodes {
        return Err(ExprError::Limit {
            nodes: facts.len(),
            depth: 1,
        });
    }
    Ok(())
}

fn insert(
    facts: &mut BTreeMap<ExprId, AbstractValue>,
    expression: ExprId,
    value: &AbstractValue,
    domain: Domain,
) {
    facts
        .entry(expression)
        .and_modify(|old| *old = domain.join(old, value))
        .or_insert_with(|| value.clone());
}
