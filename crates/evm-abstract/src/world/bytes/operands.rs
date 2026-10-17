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
    validate(&expression, value, limits)?;
    let mut clean = value.clone();
    normalize(&expression, &mut clean, domain);
    if informative(&expression, &clean) || facts.contains_key(&expression) {
        insert(facts, expression, &clean, domain);
    }
    check_count(facts.len(), limits)
}

/// Import into a logically empty map without looking up or ordering each key.
/// The private clone is published only after the original ordered checks pass.
pub(super) fn import_empty(
    source: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
    limits: ExprLimits,
) -> Result<BTreeMap<ExprId, AbstractValue>, ExprError> {
    let mut facts = source.clone();
    let mut kept = 0;
    let mut result = Ok(());
    facts.retain(|expression, value| {
        if result.is_err() {
            return true;
        }
        result = validate(expression, value, limits);
        if result.is_err() {
            return true;
        }
        normalize(expression, value, domain);
        // A physically cloned key is not a previously remembered guarantee.
        let keep = informative(expression, value);
        kept += usize::from(keep);
        // Unvisited entries are still in the clone, outside the logical prefix.
        result = check_count(kept, limits);
        keep
    });
    result?;
    Ok(facts)
}

fn validate(
    expression: &ExprId,
    value: &AbstractValue,
    limits: ExprLimits,
) -> Result<(), ExprError> {
    check_value(value, limits)?;
    if expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth {
        return Err(ExprError::Limit {
            nodes: expression.nodes(),
            depth: expression.depth(),
        });
    }
    Ok(())
}

fn normalize(expression: &ExprId, value: &mut AbstractValue, domain: Domain) {
    value.forget_identity();
    if let Some(constant) = expression.as_constant() {
        // One constant entry describes possible sources, not an occurrence of
        // a scoped input. Keep its exact numeric proof and remove must-aliases.
        *value = domain
            .join(value, &AbstractValue::constant(constant))
            .with_numeric(crate::domain::NumericValue::constant(constant));
    }
}

fn informative(expression: &ExprId, value: &AbstractValue) -> bool {
    expression
        .as_constant()
        .is_none_or(|n| *value != AbstractValue::constant(n))
}

fn check_count(count: usize, limits: ExprLimits) -> Result<(), ExprError> {
    if count > limits.max_nodes {
        return Err(ExprError::Limit {
            nodes: count,
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

#[cfg(test)]
mod tests;
