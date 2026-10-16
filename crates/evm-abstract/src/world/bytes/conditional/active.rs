//! Refine demanded COPY lengths only where old guarantees select a scope.

use super::{ConditionalFacts, Facts, Scope};
use crate::{
    domain::{
        AbstractValue,
        symbolic::{ByteStateId, ByteStateKind, ExprError, ExprId, ExprKind, ExprLimits},
    },
    resource::WorkBudget,
    world::{ByteArray, GuardRefinementError, RangeError},
};
use alloy_primitives::U256;
use std::{collections::BTreeSet, sync::Arc};

type Checked<T, E> = Result<T, GuardRefinementError<E>>;
type Reserve<E> = fn(&mut WorkBudget, usize) -> Result<(), E>;

fn cancelled<E>() -> Checked<(), E> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(RangeError::from(ExprError::Cancelled).into());
    }
    Ok(())
}

fn pay<E>(budget: &mut WorkBudget, reserve: Reserve<E>, work: usize) -> Checked<(), E> {
    cancelled()?;
    reserve(budget, work).map_err(GuardRefinementError::Proof)
}

/// Traverse byte-state edges, including captured source snapshots, without
/// visiting logical bytes or evaluating any scalar expression.
fn copy_lengths<E>(
    root: &ByteStateId,
    budget: &mut WorkBudget,
    reserve: Reserve<E>,
) -> Checked<Vec<ExprId>, E> {
    pay(budget, reserve, root.nodes())?;
    let mut pending = Vec::with_capacity(root.nodes());
    pending.push(root);
    let mut seen = BTreeSet::new();
    let mut targets: Vec<ExprId> = Vec::new();
    while let Some(state) = pending.pop() {
        // Integer navigation and insertion, including a possible new set node.
        pay(budget, reserve, seen.len().saturating_add(1))?;
        if !seen.insert(state.id()) {
            continue;
        }
        match state.kind() {
            ByteStateKind::Zero | ByteStateKind::Exact(_) | ByteStateKind::Input { .. } => {}
            ByteStateKind::WriteByte { previous, .. }
            | ByteStateKind::WriteWord { previous, .. } => pending.push(previous),
            ByteStateKind::Copy {
                previous,
                source,
                length,
                ..
            } => {
                pending.extend([previous, &source.state]);
                if !matches!(
                    length.kind(),
                    ExprKind::Operation { .. } | ExprKind::Ite { .. }
                ) {
                    continue;
                }
                let mut duplicate = false;
                for old in &targets {
                    pay(budget, reserve, length.comparison_work(old))?;
                    if length == old {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    // Reserve vector growth before retaining another target.
                    pay(budget, reserve, targets.len().saturating_add(1))?;
                    targets.push(length.clone());
                }
            }
            ByteStateKind::Slice { source } => pending.push(&source.state),
            ByteStateKind::Ite {
                then_state,
                else_state,
                ..
            } => pending.extend([then_state, else_state]),
        }
    }
    Ok(targets)
}

fn clone_work(facts: &Facts) -> usize {
    facts.iter().fold(facts.len(), |work, (expression, value)| {
        work.saturating_add(expression.work_size())
            .saturating_add(value.work_size())
    })
}

fn scope_clone_work(scope: &Scope) -> usize {
    clone_work(&scope.facts).saturating_add(scope.conditional.branches.len())
}

fn truth<E>(
    condition: &ExprId,
    facts: &Facts,
    ancestors: &[&Facts],
    budget: &mut WorkBudget,
    reserve: Reserve<E>,
) -> Checked<Option<bool>, E> {
    if let Some(value) = condition.as_constant() {
        return Ok(Some(value != U256::ZERO));
    }
    for scope in ancestors.iter().rev().copied().chain([facts]) {
        for (expression, value) in scope {
            pay(budget, reserve, condition.comparison_work(expression))?;
            if condition == expression {
                pay(budget, reserve, value.numeric().work_size())?;
                if !value.may_be_zero() {
                    return Ok(Some(true));
                }
                if !value.may_be_nonzero() {
                    return Ok(Some(false));
                }
                break;
            }
        }
    }
    Ok(None)
}

struct Refinement<'a, E, F> {
    targets: &'a [ExprId],
    limits: ExprLimits,
    budget: &'a mut WorkBudget,
    reserve: Reserve<E>,
    update: &'a mut F,
}

impl<E, F> Refinement<'_, E, F>
where
    F: FnMut(&ExprId, &mut AbstractValue, &mut WorkBudget) -> Result<bool, E>,
{
    fn pay(&mut self, work: usize) -> Checked<(), E> {
        pay(self.budget, self.reserve, work)
    }

    fn facts(&mut self, facts: &mut Facts) -> Checked<bool, E> {
        for (expression, value) in facts {
            for target in self.targets {
                self.pay(expression.comparison_work(target))?;
                if expression != target {
                    continue;
                }
                let result = (self.update)(expression, value, self.budget);
                cancelled()?;
                if !result.map_err(GuardRefinementError::Proof)? {
                    return Ok(false);
                }
                super::super::check_value(value, self.limits).map_err(RangeError::from)?;
                break;
            }
        }
        Ok(true)
    }

    fn conditional<'s>(
        &mut self,
        source: &'s ConditionalFacts,
        facts: &Facts,
        ancestors: &mut Vec<&'s Facts>,
    ) -> Checked<Option<ConditionalFacts>, E> {
        self.pay(source.branches.len())?;
        let mut branches = Vec::with_capacity(source.branches.len());
        for branch in &source.branches {
            let Some(nonzero) = truth(
                &branch.condition,
                facts,
                ancestors,
                self.budget,
                self.reserve,
            )?
            else {
                // An unknown ancestor hides the whole subtree from callbacks.
                branches.push(Arc::clone(branch));
                continue;
            };
            self.pay(
                branch
                    .then_facts
                    .facts
                    .len()
                    .saturating_add(branch.else_facts.facts.len()),
            )?;
            self.pay(
                scope_clone_work(&branch.then_facts)
                    .saturating_add(scope_clone_work(&branch.else_facts))
                    .saturating_add(1),
            )?;
            let mut out = branch.as_ref().clone();
            let (old, selected) = if nonzero {
                (&branch.then_facts, &mut out.then_facts)
            } else {
                (&branch.else_facts, &mut out.else_facts)
            };
            if !self.facts(&mut selected.facts)? {
                return Ok(None);
            }
            // Selection uses the frozen guarantees, never this pass's updates.
            ancestors.push(&old.facts);
            let nested = self.conditional(&old.conditional, facts, ancestors)?;
            ancestors.pop();
            let Some(nested) = nested else {
                return Ok(None);
            };
            selected.conditional = nested;
            branches.push(Arc::new(out));
        }
        self.pay(branches.len())?;
        let metadata_work = branches.iter().fold(branches.len(), |work, branch| {
            work.saturating_add(branch.then_facts.facts.len().saturating_mul(2))
                .saturating_add(branch.else_facts.facts.len().saturating_mul(2))
        });
        self.pay(metadata_work)?;
        let out = ConditionalFacts::from_branches(branches);
        out.check(self.limits).map_err(RangeError::from)?;
        Ok(Some(out))
    }
}

impl ByteArray {
    /// Refine retained compound COPY-length keys, including selected scopes.
    /// The caller owns the frozen proof environment and ledger errors. Every
    /// allocation is prepaid; errors or infeasibility leave this wrapper intact.
    /// Selection reads old guarantees only, and no scoped fact becomes a must fact.
    pub(crate) fn refine_active_copy_lengths<E>(
        &mut self,
        limits: ExprLimits,
        budget: &mut WorkBudget,
        reserve: Reserve<E>,
        update: &mut impl FnMut(&ExprId, &mut AbstractValue, &mut WorkBudget) -> Result<bool, E>,
    ) -> Checked<bool, E> {
        cancelled()?;
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        crate::domain::symbolic::state_check(self.root.nodes(), self.root.depth(), limits)
            .map_err(RangeError::from)?;
        self.conditional.check(limits).map_err(RangeError::from)?;
        let targets = copy_lengths(&self.root, budget, reserve)?;
        if targets.is_empty() {
            return Ok(true);
        }
        let mut refinement = Refinement {
            targets: &targets,
            limits,
            budget,
            reserve,
            update,
        };
        refinement.pay(self.operands.len())?;
        refinement.pay(clone_work(&self.operands))?;
        let mut operands = self.operands.clone();
        if !refinement.facts(&mut operands)? {
            return Ok(false);
        }
        refinement.pay(self.conditional.depth)?;
        let mut ancestors = Vec::with_capacity(self.conditional.depth);
        let Some(conditional) =
            refinement.conditional(&self.conditional, &self.operands, &mut ancestors)?
        else {
            return Ok(false);
        };
        cancelled()?;
        self.operands = operands;
        self.conditional = conditional;
        // Materialized bytes remain conservative. Reads use the immutable root
        // and updated operand bounds; refreshing each cached byte is unnecessary.
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
