//! Bounded local answers for one immutable relation snapshot within a transfer.

use crate::domain::WordElement;
use crate::domain::{
    relational::{EvmValueQuery, RelationLimits, RelationState},
    symbolic::ExprId,
};
use crate::resource::WorkBudget;
use std::collections::BTreeMap;

const MAX_ENTRIES: usize = 128;

struct Snapshot {
    state: RelationState,
    limits: RelationLimits,
}

/// Retention and lookup work are bounded independently of propagation work.
/// A cached upper approximation is independent of the caller; each caller uses its
/// own finite candidates and containment checks when intersecting the result.
pub(in crate::analysis::transfer) struct QueryCache<N: WordElement> {
    snapshot: Option<Snapshot>,
    values: BTreeMap<ExprId, EvmValueQuery<N>>,
    retained_work: usize,
    /// Maximum answer-copy quote, cached at insertion without inspecting N on lookup.
    copy_work: usize,
}

impl<N: WordElement> Default for QueryCache<N> {
    fn default() -> Self {
        Self {
            snapshot: None,
            values: BTreeMap::new(),
            retained_work: 0,
            copy_work: 0,
        }
    }
}

impl<N: WordElement> QueryCache<N> {
    /// Reserve comparison, scope replacement and possible insertion before
    /// touching the cache. Counting every entry also bounds BTreeMap searches.
    pub(super) fn work(&self, state: &RelationState, expression: &ExprId) -> usize {
        self.lookup_work(state, expression)
            .saturating_add(self.copy_work)
    }

    fn lookup_work(&self, state: &RelationState, expression: &ExprId) -> usize {
        self.retained_work
            .saturating_mul(3)
            .saturating_add(state.work_size().saturating_mul(2))
            .saturating_add(
                expression
                    .work_size()
                    .saturating_mul(self.values.len().saturating_mul(3).saturating_add(4)),
            )
            .saturating_add(1)
    }

    fn scope(&mut self, state: &RelationState, limits: &RelationLimits) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.limits != *limits || snapshot.state != *state)
        {
            self.snapshot = None;
            self.values.clear();
            self.retained_work = 0;
            self.copy_work = 0;
        }
    }

    pub(super) fn get(
        &mut self,
        state: &RelationState,
        expression: &ExprId,
        limits: &RelationLimits,
        budget: &mut WorkBudget,
    ) -> Option<EvmValueQuery<N>> {
        // Caching is optional: an unaffordable lookup/copy remains a miss and
        // must not poison the ledger before the actual query is attempted.
        let work = self.work(state, expression);
        if work == usize::MAX || !budget.can_afford(work) || !budget.charge(work) {
            return None;
        }
        self.scope(state, limits);
        self.values.get(expression).cloned()
    }

    pub(super) fn remember(
        &mut self,
        state: &RelationState,
        expression: &ExprId,
        limits: &RelationLimits,
        answer: &EvmValueQuery<N>,
        budget: &mut WorkBudget,
    ) {
        if matches!(answer, EvmValueQuery::Undetermined(_)) || !limits.enabled || !limits.valid() {
            return;
        }
        let state_work = state.work_size();
        let entry_work = expression
            .work_size()
            .saturating_add(1)
            .saturating_add(answer_work(answer));
        if state_work.saturating_add(entry_work) > limits.max_nodes {
            return;
        }
        let copy_work = answer_copy_work(answer);
        // Pay comparison, possible eviction, snapshot/key and numeric copies
        // before scope replacement or answer.clone(). Declining retention has
        // no effect on the already computed sound query answer.
        let work = self
            .lookup_work(state, expression)
            .saturating_add(state_work)
            .saturating_add(expression.work_size())
            .saturating_add(copy_work);
        if work == usize::MAX || !budget.can_afford(work) || !budget.charge(work) {
            return;
        }
        self.scope(state, limits);
        if self.snapshot.is_none() {
            self.snapshot = Some(Snapshot {
                state: state.clone(),
                limits: *limits,
            });
            self.retained_work = state_work;
        }
        if self.values.contains_key(expression) {
            return;
        }
        if self.values.len() >= MAX_ENTRIES
            || self.retained_work.saturating_add(entry_work) > limits.max_nodes
        {
            // Eviction only forgets an optional proof; the next query still
            // runs bounded local propagation under the same policy.
            self.values.clear();
            self.retained_work = state_work;
            self.copy_work = 0;
        }
        self.values.insert(expression.clone(), answer.clone());
        self.retained_work = self.retained_work.saturating_add(entry_work);
        self.copy_work = self.copy_work.max(copy_work);
    }
}

fn answer_work<N: WordElement>(answer: &EvmValueQuery<N>) -> usize {
    match answer {
        EvmValueQuery::Bounds(value) => value.work_size().saturating_add(1),
        EvmValueQuery::Exact(_)
        | EvmValueQuery::ProvenImpossible
        | EvmValueQuery::Undetermined(_) => 1,
    }
}

fn answer_copy_work<N: WordElement>(answer: &EvmValueQuery<N>) -> usize {
    match answer {
        EvmValueQuery::Bounds(value) => value.clone_work().saturating_add(1),
        EvmValueQuery::Exact(_)
        | EvmValueQuery::ProvenImpossible
        | EvmValueQuery::Undetermined(_) => 1,
    }
}

#[cfg(test)]
mod tests;
