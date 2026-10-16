//! Summary traversal includes source tokens hidden inside byte roots.

use super::{ByteArray, RangeError};
use crate::domain::{
    AbstractValue,
    symbolic::{ByteStateId, ByteStateKind, ExprId, ExprLimits},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn expressions(root: &ByteStateId) -> Vec<ExprId> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(state) = pending.pop() {
        if !seen.insert(state.id()) {
            continue;
        }
        match state.kind() {
            ByteStateKind::Zero | ByteStateKind::Exact(_) => {}
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => {
                out.extend([source.token().clone(), length.clone()]);
                out.extend(bindings.iter().map(|b| b.value.clone()));
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            }
            | ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                pending.push(previous.clone());
                out.extend([offset.clone(), value.clone()]);
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                pending.extend([previous.clone(), source.state.clone()]);
                out.extend([
                    destination.clone(),
                    source.offset.clone(),
                    source.length.clone(),
                    length.clone(),
                ]);
            }
            ByteStateKind::Slice { source } => {
                pending.push(source.state.clone());
                out.extend([source.offset.clone(), source.length.clone()]);
            }
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                pending.extend([then_state.clone(), else_state.clone()]);
                out.push(condition.clone());
            }
        }
    }
    out
}
fn rename_value(
    value: &mut AbstractValue,
    map: &BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
) -> Result<(), RangeError> {
    if let Some(expression) = value.expression() {
        *value = value
            .clone()
            .with_expression(expression.rename_fresh(map, limits)?);
    }
    Ok(())
}
impl ByteArray {
    /// All fresh leaves, including hidden input/source tokens and range operands.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut out = self.root.fresh_leaves();
        self.visit_values(&mut |v| {
            if let Some(e) = v.expression() {
                out.extend(e.fresh_leaves());
            }
        });
        out
    }
    /// One capture-avoiding map must be shared by the entire machine payload.
    pub fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), RangeError> {
        let mut out = self.clone();
        out.root = out.root.rename_fresh(map, limits)?;
        out.allocations = self
            .allocations
            .iter()
            .map(|(at, size)| {
                Ok((
                    at.rename_fresh(map, limits)?,
                    size.rename_fresh(map, limits)?,
                ))
            })
            .collect::<Result<_, RangeError>>()?;
        rename_value(&mut out.length, map, limits)?;
        rename_value(&mut out.default, map, limits)?;
        for v in out.bytes.values_mut() {
            rename_value(v, map, limits)?;
        }
        out.operands.clear();
        for (e, v) in &self.operands {
            let mut v = v.clone();
            rename_value(&mut v, map, limits)?;
            super::operands::remember(
                &mut out.operands,
                e.rename_fresh(map, limits)?,
                &v,
                crate::domain::Domain::default(),
                limits,
            )?;
        }
        out.conditional = self.conditional.map(&mut |expression, value| {
            let mut value = value.cloned();
            if let Some(value) = &mut value {
                rename_value(value, map, limits)?;
            }
            Ok((expression.rename_fresh(map, limits)?, value))
        })?;
        out.conditional.check(limits)?;
        *self = out;
        Ok(())
    }
    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(&AbstractValue)) {
        visit(&self.length);
        visit(&self.default);
        for v in self.bytes.values().chain(self.operands.values()) {
            visit(v);
        }
        self.conditional.visit(&mut |expression, value| {
            if value.is_none_or(|value| value.expression() != Some(expression)) {
                visit(&AbstractValue::top().with_expression(expression.clone()));
            }
            if let Some(value) = value {
                visit(value);
            }
        });
        for (at, size) in &self.allocations {
            visit(&AbstractValue::top().with_expression(at.clone()));
            visit(&AbstractValue::top().with_expression(size.clone()));
        }
        for e in expressions(&self.root) {
            visit(&AbstractValue::top().with_expression(e));
        }
    }
    /// Update unconditional materialized values. A callback may reject the whole
    /// payload, so branch-local operands must stay outside this API. Their bounds
    /// remain scoped; reads consult retained selector truths through Context.
    /// Symbolic source identity changes require rename_fresh.
    pub(crate) fn update_values(&mut self, update: &mut impl FnMut(&mut AbstractValue)) {
        let length_expression = self.length.symbolic_expression();
        update(&mut self.length);
        if self.length.symbolic_expression().is_none()
            && let Some(expression) = length_expression
        {
            self.length = self.length.clone().with_expression(expression);
        }
        update(&mut self.default);
        for v in self.bytes.values_mut().chain(self.operands.values_mut()) {
            update(v);
        }
    }
    /// Refine retained numeric guarantees against their original source terms.
    /// Operand values deliberately forget source identity; the map key keeps
    /// the expression needed to refine copy offsets, lengths, and selectors.
    /// Only unconditional entries participate: a contradiction with an inactive
    /// conditional source cannot justify rejecting the entire payload.
    pub(crate) fn refine_operands(&mut self, update: &mut impl FnMut(&ExprId, &mut AbstractValue)) {
        for (expression, value) in &mut self.operands {
            update(expression, value);
        }
    }
}

/// Charge retained concrete payloads once per shared state, independently of
/// logical copy lengths and without traversing any payload bytes.
pub(super) fn exact_work(root: &ByteStateId) -> usize {
    let mut work = 0usize;
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(state) = pending.pop() {
        if !seen.insert(state.id()) {
            continue;
        }
        match state.kind() {
            ByteStateKind::Exact(bytes) => work = work.saturating_add(bytes.len()),
            ByteStateKind::WriteByte { previous, .. }
            | ByteStateKind::WriteWord { previous, .. } => pending.push(previous.clone()),
            ByteStateKind::Copy {
                previous, source, ..
            } => pending.extend([previous.clone(), source.state.clone()]),
            ByteStateKind::Slice { source } => pending.push(source.state.clone()),
            ByteStateKind::Ite {
                then_state,
                else_state,
                ..
            } => pending.extend([then_state.clone(), else_state.clone()]),
            _ => {}
        }
    }
    work
}
