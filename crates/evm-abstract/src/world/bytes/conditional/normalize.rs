//! Exact shared alternatives of complete branch-local implication scopes.
//!
//! A scope's facts are a prefix of its conditional conjunction. Only an empty
//! prefix followed by one branch exposes an ITE; every other scope is opaque.
//! No byte-state identity or numeric-only equality participates in these rules.

use super::{Branch, ConditionalFacts, Scope};
use crate::domain::WordElement;
use crate::domain::symbolic::{ExprError, ExprId, ExprLimits};
use alloy_primitives::U256;
use std::sync::Arc;

/// Spend at most two metadata passes from the caller's existing four-pass join
/// construction/retention precharge. The original scope copies and metric scan
/// keep their two passes. Work includes expanded guards, fact keys and complete
/// values, so equality is variable work even when byte roots happen to agree.
/// Refusal discards the whole optional attempt, including any earlier steps.
struct Actions {
    remaining: usize,
    declined: bool,
}

impl Actions {
    fn pay(&mut self, work: usize) -> Result<bool, ExprError> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled);
        }
        if self.declined || work > self.remaining {
            self.declined = true;
            return Ok(false);
        }
        self.remaining -= work;
        Ok(true)
    }

    fn equal<N: WordElement>(
        &mut self,
        left: &Scope<N>,
        right: &Scope<N>,
    ) -> Result<bool, ExprError> {
        if !self.pay(1)? || left.facts.len() != right.facts.len() {
            return Ok(false);
        }
        // Pay the shallow quotation scan before touching its entries. Paired
        // equality visits at most the left expanded structure: unequal lengths
        // stop, and equal lengths compare keys and every AbstractValue layer.
        if !self.pay(left.facts.len())? {
            return Ok(false);
        }
        for (expression, value) in &left.facts {
            if !self.pay(expression.work_size().saturating_add(value.work_size()))? {
                return Ok(false);
            }
        }
        if !self.pay(left.conditional.work.saturating_add(1))? {
            return Ok(false);
        }
        Ok(left == right)
    }

    fn cloned<N: WordElement>(&mut self, scope: &Scope<N>) -> Result<Option<Scope<N>>, ExprError> {
        // One shallow quote, key/entry copies and the final from_branches fact
        // scan are funded here. Conditional Clone copies only top-level Arcs;
        // nested facts remain immutable and shared, without a subtree census.
        if !self.pay(
            scope
                .facts
                .len()
                .saturating_mul(4)
                .saturating_add(scope.conditional.branches.len())
                .saturating_add(1),
        )? {
            return Ok(None);
        }
        for value in scope.facts.values() {
            if !self.pay(value.work_size())? {
                return Ok(None);
            }
        }
        Ok(Some(scope.clone()))
    }
}

fn nested<N: WordElement>(scope: &Scope<N>) -> Option<&Branch<N>> {
    if scope.facts.is_empty() && scope.conditional.branches.len() == 1 {
        Some(&scope.conditional.branches[0])
    } else {
        None
    }
}

type SharedScopes<'a, N> = (ExprId, ExprId, &'a Scope<N>, &'a Scope<N>);

fn shared<'a, N: WordElement>(
    yes: &'a Scope<N>,
    no: &'a Scope<N>,
    actions: &mut Actions,
) -> Result<Option<SharedScopes<'a, N>>, ExprError> {
    let yes_ite = nested(yes);
    let no_ite = nested(no);
    if let (Some(g), Some(h)) = (yes_ite, no_ite)
        && actions.equal(&g.then_facts, &h.then_facts)?
        && actions.equal(&g.else_facts, &h.else_facts)?
    {
        // I(s,I(g,A,B),I(h,A,B)) = I(I(s,g,h),A,B).
        return Ok(Some((
            g.condition.clone(),
            h.condition.clone(),
            &g.then_facts,
            &g.else_facts,
        )));
    }
    if let Some(t) = yes_ite {
        let selected = if actions.equal(no, &t.then_facts)? {
            Some(U256::ONE)
        } else if actions.equal(no, &t.else_facts)? {
            Some(U256::ZERO)
        } else {
            None
        };
        if let Some(selected) = selected {
            // I(s,I(t,A,B),A/B) = I(I(s,t,1/0),A,B).
            return Ok(Some((
                t.condition.clone(),
                ExprId::constant(selected),
                &t.then_facts,
                &t.else_facts,
            )));
        }
    }
    if let Some(t) = no_ite {
        let selected = if actions.equal(yes, &t.then_facts)? {
            Some(U256::ONE)
        } else if actions.equal(yes, &t.else_facts)? {
            Some(U256::ZERO)
        } else {
            None
        };
        if let Some(selected) = selected {
            // I(s,A/B,I(t,A,B)) = I(I(s,1/0,t),A,B).
            return Ok(Some((
                ExprId::constant(selected),
                t.condition.clone(),
                &t.then_facts,
                &t.else_facts,
            )));
        }
    }
    Ok(None)
}

pub(super) fn shared_branches<N: WordElement>(
    original: ConditionalFacts<N>,
    limits: ExprLimits,
) -> Result<ConditionalFacts<N>, ExprError> {
    let branch = &original.branches[0];
    // Cached input delta work excludes the new outer selector. It is bounded
    // by the two input ByteArray work sizes already charged by machine joins.
    let mut actions = Actions {
        remaining: original
            .work
            .saturating_sub(branch.condition.work_size())
            .saturating_mul(2),
        declined: false,
    };
    let mut condition = branch.condition.clone();
    let mut yes = &branch.then_facts;
    let mut no = &branch.else_facts;
    let mut changed = false;
    loop {
        if !actions.pay(1)? {
            return Ok(original);
        }
        let Some((g, h, a, b)) = shared(yes, no, &mut actions)? else {
            break;
        };
        let nodes = condition
            .nodes()
            .saturating_add(g.nodes())
            .saturating_add(h.nodes())
            .saturating_add(1);
        let depth = condition
            .depth()
            .max(g.depth())
            .max(h.depth())
            .saturating_add(1);
        // A/B are original descendant scopes: their fact and conditional depths
        // are at most original.depth - 1, and retained entries only decrease.
        // The derived scalar guard is the only dimension that can grow. These
        // cached bounds precede construction, without suppressing any error.
        if nodes == usize::MAX
            || nodes > limits.max_nodes
            || depth.saturating_add(1) == usize::MAX
            || depth.saturating_add(1) > limits.max_depth
            || !actions.pay(nodes)?
        {
            return Ok(original);
        }
        condition = ExprId::ite(condition, g, h, limits)?;
        yes = a;
        no = b;
        changed = true;
        // Each iteration removes an exposed metadata ITE. No Boolean search,
        // common-fact promotion or traversal of opaque scopes occurs here.
    }
    if !changed || actions.declined || !actions.pay(3)? {
        return Ok(original);
    }
    let Some(then_facts) = actions.cloned(yes)? else {
        return Ok(original);
    };
    let Some(else_facts) = actions.cloned(no)? else {
        return Ok(original);
    };
    let out = ConditionalFacts::<N>::from_branches(vec![Arc::new(Branch {
        condition,
        then_facts,
        else_facts,
    })]);
    // Native serialization has its own frozen policy, which can be tighter
    // than the caller's ExprLimits. Compare cached dimensions, without invoking
    // serialization or swallowing cancellation from a real metadata check.
    let native = crate::domain::symbolic::GraphLimits::default();
    let fits_native = |metadata: &ConditionalFacts<N>| {
        !metadata.limited
            && metadata.entries.max(metadata.largest_expression) <= native.max_nodes
            && metadata.depth <= native.max_depth
            && metadata.work <= native.max_nodes
    };
    if fits_native(&original) && !fits_native(&out) {
        return Ok(original);
    }
    if out.work > limits.max_nodes && original.work <= limits.max_nodes {
        return Ok(original);
    }
    out.check(limits)?;
    Ok(out)
}

#[cfg(test)]
mod tests;
