//! Conservative metadata form for explicitly selected whole-case cycle widening.

use super::{
    ByteArray, RangeError,
    refinement::{Node, walk},
};
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{ExprError, ExprId, ExprLimits},
};
use std::collections::BTreeMap;

/// Constant folding after the caller's registered-selector projection is an
/// authoritative proof, while nonconstant external/manual symbols stay intact.
pub(in crate::world) fn refresh_constant(value: &mut AbstractValue) {
    if let Some(constant) = value.expression().and_then(ExprId::as_constant) {
        *value = value
            .clone()
            .with_numeric(crate::domain::NumericValue::constant(constant));
    }
}

/// Retain guarantees consumed by the authoritative graph, including guarantees
/// referenced by another retained fact. Constants need no duplicate binding.
pub(in crate::world) fn retained_facts(
    pending: Vec<Node>,
    facts: &BTreeMap<ExprId, AbstractValue>,
    limits: ExprLimits,
) -> Result<BTreeMap<ExprId, AbstractValue>, ExprError> {
    let mut retained = BTreeMap::new();
    let mut work = 0usize;
    let nodes = walk(pending, limits, &mut |node, pending| {
        if let Node::Expression(expression) = node
            && expression.as_constant().is_none()
            && let Some(value) = facts.get(expression)
        {
            work = work
                .saturating_add(value.numeric().work_size())
                .saturating_add(1);
            if work > limits.max_nodes {
                return Err(ExprError::Limit {
                    nodes: work,
                    depth: expression.depth(),
                });
            }
            if let Some(value_expression) = value.expression() {
                pending.push(Node::Expression(value_expression.clone()));
            }
            retained.insert(expression.clone(), value.clone());
        }
        Ok(())
    })?;
    let nodes = nodes.saturating_add(work);
    if nodes > limits.max_nodes {
        return Err(ExprError::Limit { nodes, depth: 1 });
    }
    Ok(retained)
}

impl ByteArray {
    /// Canonicalize disposable cycle metadata without rewriting immutable
    /// content, external operand guarantees, source identity, or logical length.
    pub(crate) fn widening_form(
        &self,
        _domain: Domain,
        limits: ExprLimits,
    ) -> Result<Self, RangeError> {
        self.check_existing(limits)?;
        let mut pending = vec![Node::Bytes(self.root.clone())];
        if let Some(length) = self.length.expression() {
            pending.push(Node::Expression(length.clone()));
        }
        let mut out = Self {
            length: self.length.clone(),
            bytes: BTreeMap::new(),
            default: AbstractValue::unknown_byte(),
            memory: self.memory,
            root: self.root.clone(),
            allocations: Default::default(),
            operands: retained_facts(pending, &self.operands, limits)?,
            failure: None,
        };
        refresh_constant(&mut out.length);
        out.check_existing(limits)?;
        Ok(out)
    }
}
