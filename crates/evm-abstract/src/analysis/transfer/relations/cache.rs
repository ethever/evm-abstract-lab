//! Exact native answers for one immutable relation snapshot within a transfer.

use crate::domain::{
    relational::{RelationLimits, RelationState, ValueQuery},
    symbolic::ExprId,
};
use std::collections::BTreeMap;

const MAX_ENTRIES: usize = 128;

struct Snapshot {
    state: RelationState,
    limits: RelationLimits,
}

/// Retention and lookup work are bounded independently of native solver fuel.
/// No numeric approximation is cached: each caller applies the proof to its
/// current value, including its own finite candidates and containment checks.
#[derive(Default)]
pub(in crate::analysis::transfer) struct QueryCache {
    snapshot: Option<Snapshot>,
    values: BTreeMap<ExprId, ValueQuery>,
    retained_work: usize,
}

impl QueryCache {
    /// Reserve comparison, scope replacement and possible insertion before
    /// touching the cache. Counting every entry also bounds BTreeMap searches.
    pub(super) fn work(&self, state: &RelationState, expression: &ExprId) -> usize {
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
        }
    }

    pub(super) fn get(
        &mut self,
        state: &RelationState,
        expression: &ExprId,
        limits: &RelationLimits,
    ) -> Option<ValueQuery> {
        self.scope(state, limits);
        self.values.get(expression).cloned()
    }

    pub(super) fn remember(
        &mut self,
        state: &RelationState,
        expression: &ExprId,
        limits: &RelationLimits,
        answer: &ValueQuery,
    ) {
        self.scope(state, limits);
        if matches!(answer, ValueQuery::Unknown(_)) || !limits.enabled || !limits.valid() {
            return;
        }
        let state_work = state.work_size();
        let entry_work = expression.work_size().saturating_add(1);
        if state_work.saturating_add(entry_work) > limits.max_nodes {
            return;
        }
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
            // follows the complete native encoding and unchanged allowance.
            self.values.clear();
            self.retained_work = state_work;
        }
        self.values.insert(expression.clone(), answer.clone());
        self.retained_work = self.retained_work.saturating_add(entry_work);
    }
}

#[cfg(test)]
mod tests;
