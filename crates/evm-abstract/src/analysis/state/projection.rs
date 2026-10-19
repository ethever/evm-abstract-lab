//! Import local proved scalar bounds before discarding relational history.
//!
//! This is a numerical overapproximation, with no native SMT/model extraction.
//! Summary import memoizes numerical upper bounds; branch metadata keeps its
//! feasibility checks and precision diagnostics. Both share query accounting.

use super::{
    AbstractValue, ByteArray, Domain, MachineError, MachinePayload, NumericValue, RelationState,
    WorkBudget,
};
use crate::domain::{
    relational::{QueryReason, RelationLimits, ScalarQuery},
    symbolic::{ExprId, ExprKind},
};
use crate::{
    analysis::{DiagnosticKind, state::StatePart},
    world::Store,
};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Summary import may memoize an optional upper bound; branch metadata must
/// retain each occurrence's query fee, infeasibility and precision diagnostics.
pub(in crate::analysis) enum Policy<'a> {
    Summary,
    Branch {
        diagnostics: &'a mut Vec<(usize, DiagnosticKind)>,
        pc: usize,
    },
}

pub(in crate::analysis) struct Projection<'a> {
    relations: &'a RelationState,
    relations_work_size: Option<usize>,
    #[cfg(test)]
    relations_work_size_scans: usize,
    domain: Domain,
    limits: &'a RelationLimits,
    budget: &'a mut WorkBudget,
    cache: BTreeMap<ExprId, Vec<(NumericValue, Option<NumericValue>)>>,
    policy: Policy<'a>,
    /// False is a proved infeasible branch; summary import never rejects a path.
    outcome: Result<bool, MachineError>,
}

impl<'a> Projection<'a> {
    pub(in crate::analysis) fn new(
        relations: &'a RelationState,
        domain: Domain,
        limits: &'a RelationLimits,
        budget: &'a mut WorkBudget,
        policy: Policy<'a>,
    ) -> Self {
        Self {
            relations,
            relations_work_size: None,
            #[cfg(test)]
            relations_work_size_scans: 0,
            domain,
            limits,
            budget,
            cache: BTreeMap::new(),
            policy,
            outcome: Ok(true),
        }
    }

    fn reserve(&mut self, cost: usize) -> bool {
        if self.budget.charge(cost) {
            return true;
        }
        // Branch metadata preserves the transfer's Work boundary. Summary
        // construction also checks cancellation when recording a refusal.
        let error = if matches!(&self.policy, Policy::Summary) {
            MachineError::work_failure()
        } else {
            MachineError::Work
        };
        self.outcome = Err(error);
        false
    }

    fn expression(&mut self, expression: &ExprId, value: &mut AbstractValue) {
        if !matches!(self.outcome, Ok(true))
            || value.singleton().is_some()
            || self.relations.is_empty()
        {
            return;
        }
        let summary = matches!(&self.policy, Policy::Summary);
        if summary
            && let Some(cached) = self.cache.get(expression)
            && let Some((_, refined)) = cached.iter().find(|(old, _)| old == value.numeric())
        {
            if let Some(refined) = refined {
                *value = value.clone().with_numeric(refined.clone());
            }
            return;
        }
        let relation_work = *self.relations_work_size.get_or_insert_with(|| {
            #[cfg(test)]
            {
                self.relations_work_size_scans += 1;
            }
            self.relations.work_size()
        });
        if !self.reserve(expression.work_size().saturating_add(relation_work)) {
            return;
        }
        let relevant = if summary {
            self.relations.has_direct_numeric_guarantee(expression)
        } else {
            self.relations.relevant(expression)
        };
        if !relevant || !self.reserve(self.domain.refinement_work(value, self.relations.len())) {
            return;
        }
        let old = summary.then(|| value.numeric().clone());
        let refined = match self.relations.refine_numeric(
            expression,
            value.numeric(),
            self.domain,
            self.limits,
        ) {
            ScalarQuery::Refined { numeric, .. } => Some(*numeric),
            ScalarQuery::Infeasible if !summary => {
                self.outcome = Ok(false);
                return;
            }
            ScalarQuery::Unknown(QueryReason::Cancelled) => {
                self.outcome = Err(MachineError::Relations(QueryReason::Cancelled));
                return;
            }
            ScalarQuery::Unknown(reason) if !summary && reason != QueryReason::ScalarFactLimit => {
                if let Policy::Branch { diagnostics, pc } = &mut self.policy {
                    diagnostics.push((*pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                }
                None
            }
            // Contradictory or unsupported optional summary import keeps the old
            // upper bound. Branch metadata alone may reject an infeasible path.
            ScalarQuery::Infeasible | ScalarQuery::Unchanged | ScalarQuery::Unknown(_) => None,
        };
        if let Some(refined) = &refined {
            *value = value.clone().with_numeric(refined.clone());
        }
        if let Some(old) = old {
            let cache = self.cache.entry(expression.clone()).or_default();
            if let Some(refined) = &refined
                && refined != &old
            {
                cache.push((refined.clone(), Some(refined.clone())));
            }
            cache.push((old, refined));
        }
    }

    pub(in crate::analysis) fn value(&mut self, value: &mut AbstractValue) {
        if let Some(expression) = value.expression().cloned() {
            self.expression(&expression, value);
        }
    }

    pub(in crate::analysis) fn bytes(&mut self, bytes: &mut ByteArray) {
        let summary = matches!(&self.policy, Policy::Summary);
        bytes.update_values(&mut |value| {
            if summary
                || !value.expression().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        ExprKind::ByteRead { .. }
                            | ExprKind::WordRead { .. }
                            | ExprKind::Keccak { .. }
                    )
                })
            {
                self.value(value);
            }
        });
        bytes.refine_operands(&mut |expression, value| self.expression(expression, value));
    }

    pub(in crate::analysis) fn store(&mut self, store: &mut Store) {
        store.update_values(&mut |value| self.value(value));
        store.refine_operands(&mut |expression, value| self.expression(expression, value));
    }

    pub(in crate::analysis) fn finish(self) -> Result<bool, MachineError> {
        self.outcome
    }

    pub(in crate::analysis) fn payload(
        &mut self,
        payload: &mut MachinePayload,
    ) -> Result<(), MachineError> {
        self.store(&mut payload.store);
        payload.call_stack.transform(&mut |part| {
            match part {
                StatePart::Value(value) => self.value(value),
                StatePart::Bytes(bytes) => self.bytes(bytes),
                StatePart::Store(store) => self.store(store.to_mut()),
            }
            Ok(())
        })
    }
}
