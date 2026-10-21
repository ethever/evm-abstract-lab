//! Import local proved scalar bounds before discarding relational history.
//!
//! This is a numerical overapproximation, with no native SMT/model extraction.
//! Summary import memoizes numerical upper bounds; branch metadata keeps its
//! feasibility checks and precision diagnostics. Both share query accounting.

use crate::domain::EvmNumericAlgebra;

use super::{
    AlgebraRef, EvmByteArray, EvmMachinePayload, EvmValue, MachineError, RelationState, WorkBudget,
};
use crate::domain::{
    relational::{EvmScalarQuery, QueryReason, RelationLimits},
    symbolic::{ExprId, ExprKind},
};
use crate::{
    analysis::{DiagnosticKind, state::StatePart},
    world::EvmStore,
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

type ProjectionCache<N> = BTreeMap<ExprId, Vec<(N, Option<N>)>>;

pub(in crate::analysis) struct Projection<'a, D: EvmNumericAlgebra> {
    relations: &'a RelationState,
    relations_work_size: Option<usize>,
    #[cfg(test)]
    relations_work_size_scans: usize,
    domain: AlgebraRef<'a, D>,
    limits: &'a RelationLimits,
    budget: &'a mut WorkBudget,
    cache: ProjectionCache<D::Element>,
    policy: Policy<'a>,
    /// False is a proved infeasible branch; summary import never rejects a path.
    outcome: Result<bool, MachineError>,
}

impl<'a, D: EvmNumericAlgebra> Projection<'a, D> {
    pub(in crate::analysis) fn new(
        relations: &'a RelationState,
        domain: AlgebraRef<'a, D>,
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

    /// Branch callers stop between metadata segments; summary import retains
    /// its original whole-state traversal even after optional import fails.
    pub(in crate::analysis) fn stopped(&self) -> bool {
        !matches!(self.outcome, Ok(true))
    }

    fn expression(&mut self, expression: &ExprId, value: &mut EvmValue<D::Element>) {
        // A retained literal is already a proof about this source. Numeric
        // singleton discovery is a different, separately priced operation.
        if self.stopped() || expression.as_constant().is_some() || self.relations.is_empty() {
            return;
        }
        let summary = matches!(&self.policy, Policy::Summary);
        if summary
            && let Some(cached) = self.cache.get(expression)
            && let Some((_, refined)) = cached.iter().find(|(old, _)| old == value.numeric())
        {
            if let Some(refined) = refined {
                *value = value
                    .clone()
                    .with_numeric_after_singleton(refined.clone(), None);
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
        if !relevant {
            return;
        }
        let answer = match self.relations.refine_numeric_budgeted_with(
            expression,
            value,
            self.domain,
            self.limits,
            self.budget,
        ) {
            Ok(answer) => answer,
            Err(_) => {
                self.outcome = Err(if summary {
                    MachineError::work_failure()
                } else {
                    MachineError::Work
                });
                return;
            }
        };
        // The import/whole reservation precedes this optional cache copy too.
        let old = summary.then(|| value.numeric().clone());
        let refined = match answer {
            EvmScalarQuery::Refined { numeric, .. } => Some(*numeric),
            EvmScalarQuery::Infeasible if !summary => {
                self.outcome = Ok(false);
                return;
            }
            EvmScalarQuery::Unknown(QueryReason::Cancelled) => {
                self.outcome = Err(MachineError::Relations(QueryReason::Cancelled));
                return;
            }
            EvmScalarQuery::Unknown(reason)
                if !summary && reason != QueryReason::ScalarFactLimit =>
            {
                if let Policy::Branch { diagnostics, pc } = &mut self.policy {
                    diagnostics.push((*pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                }
                None
            }
            // Contradictory or unsupported optional summary import keeps the old
            // upper bound. Branch metadata alone may reject an infeasible path.
            EvmScalarQuery::Infeasible | EvmScalarQuery::Unchanged | EvmScalarQuery::Unknown(_) => {
                None
            }
        };
        if let Some(refined) = &refined {
            // The refinement operation owns any discovery. Applying its bound,
            // including a cached one, must not run an opaque metadata query.
            *value = value
                .clone()
                .with_numeric_after_singleton(refined.clone(), None);
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

    pub(in crate::analysis) fn value(&mut self, value: &mut EvmValue<D::Element>) {
        if let Some(expression) = value.expression().cloned() {
            self.expression(&expression, value);
        }
    }

    pub(in crate::analysis) fn bytes(&mut self, bytes: &mut EvmByteArray<D::Element>) {
        let summary = matches!(&self.policy, Policy::Summary);
        bytes.update_values(&mut |value| {
            if summary
                || (!self.stopped()
                    && !value.expression().is_some_and(|e| {
                        matches!(
                            e.kind(),
                            ExprKind::ByteRead { .. }
                                | ExprKind::WordRead { .. }
                                | ExprKind::Keccak { .. }
                        )
                    }))
            {
                self.value(value);
            }
        });
        if summary || !self.stopped() {
            bytes.refine_operands(&mut |expression, value| self.expression(expression, value));
        }
    }

    pub(in crate::analysis) fn store(&mut self, store: &mut EvmStore<D::Element>) {
        store.update_values(&mut |value| self.value(value));
        store.refine_operands(&mut |expression, value| self.expression(expression, value));
    }

    pub(in crate::analysis) fn finish(self) -> Result<bool, MachineError> {
        self.outcome
    }

    pub(in crate::analysis) fn payload(
        &mut self,
        payload: &mut EvmMachinePayload<D::Element>,
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
