//! Distribute a proved composite length bound without selecting any guard.

use super::{Branch, ConditionalFacts, Facts, Scope};
use crate::{
    domain::{
        AbstractValue,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprKind, ExprLimits},
    },
    resource::WorkBudget,
    world::ByteArray,
};
use alloy_primitives::U256;
use std::sync::Arc;

const IMPORT_WORK: usize = 16 * 1024;

fn cancelled() -> Result<(), QueryReason> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(QueryReason::Cancelled);
    }
    Ok(())
}

fn empty() -> Scope {
    Scope {
        facts: Facts::new(),
        conditional: ConditionalFacts::default(),
    }
}

/// F proves the whole expression belongs to bound. For an ITE this proves
/// each arm's bound only under that arm's selector and all ancestor selectors.
/// Non-ITE definitions are opaque: there is no inference about their operands.
fn distribute(expression: &ExprId, bound: &AbstractValue) -> Scope {
    if let ExprKind::Ite {
        condition,
        then_value,
        else_value,
    } = expression.kind()
    {
        Scope {
            facts: Facts::new(),
            conditional: ConditionalFacts::from_branches(vec![Arc::new(Branch {
                condition: condition.clone(),
                then_facts: distribute(then_value, bound),
                else_facts: distribute(else_value, bound),
            })]),
        }
    } else {
        Scope {
            facts: Facts::from([(expression.clone(), bound.clone())]),
            conditional: ConditionalFacts::default(),
        }
    }
}

impl ByteArray {
    /// Import only caller-proved numeric bounds of this frame's exact length
    /// expression. Content roots, allocations, source handles and frozen reads
    /// are untouched. False means an optional update was declined atomically.
    pub(crate) fn import_length_bound(
        &mut self,
        length: &AbstractValue,
        limits: ExprLimits,
        budget: &mut WorkBudget,
    ) -> Result<bool, QueryReason> {
        cancelled()?;
        let Some(expression) = length.expression() else {
            return Ok(false);
        };
        if self.failure.is_some() || length.numeric().is_top() || length.symbolic_limit_reached() {
            return Ok(false);
        }
        // Cached expanded sizes bound the ITE walk, fact copies, stamp lookup,
        // and rebuilding top-level branch metrics. No byte-state root is read.
        let nodes = expression.nodes();
        let entries = self
            .conditional
            .len()
            .saturating_add(nodes.saturating_mul(3))
            .saturating_add(3);
        let retained = self
            .conditional
            .work_size()
            .saturating_add(nodes.saturating_mul(2))
            .saturating_add(nodes.saturating_add(1).saturating_mul(length.work_size()))
            .saturating_add(1);
        let work = nodes
            .saturating_add(self.conditional.work_size())
            .saturating_add(1)
            .saturating_mul(length.work_size().saturating_add(nodes).saturating_add(8))
            .saturating_mul(4);
        if nodes > limits.max_nodes
            || entries > limits.max_nodes
            || retained > limits.max_nodes
            || expression.depth().saturating_add(1) > limits.max_depth
            || self.conditional.check(limits).is_err()
            || work > IMPORT_WORK
            || !budget.can_afford(work)
        {
            cancelled()?;
            return Ok(false);
        }
        if !budget.charge(work) {
            cancelled()?;
            return Ok(false);
        }
        let mut bound = length.clone();
        bound.forget_identity();
        bound.forget_expression();
        let mut scope = distribute(expression, &bound);
        // F proves the composite itself unconditionally. Its leaf implications
        // remain guarded. An older whole-R fact alone is not an import stamp.
        scope.facts.insert(expression.clone(), bound);
        // Match the complete implication tree. Nested stamps stay branch-local
        // after joins and cannot suppress an unconditional import.
        if self.conditional.branches.iter().any(|branch| {
            branch.condition.as_constant() == Some(U256::ONE)
                && branch.else_facts.facts.is_empty()
                && branch.else_facts.conditional.branches.is_empty()
                && branch.then_facts == scope
        }) {
            cancelled()?;
            return Ok(true);
        }
        let mut branches = self.conditional.branches.clone();
        branches.push(Arc::new(Branch {
            condition: ExprId::constant(U256::ONE),
            then_facts: scope,
            else_facts: empty(),
        }));
        let out = ConditionalFacts::from_branches(branches);
        match out.check(limits) {
            Err(ExprError::Cancelled) => return Err(QueryReason::Cancelled),
            Err(_) => return Ok(false),
            Ok(()) => {}
        }
        if out.work_size() > limits.max_nodes {
            return Ok(false);
        }
        cancelled()?;
        self.conditional = out;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
