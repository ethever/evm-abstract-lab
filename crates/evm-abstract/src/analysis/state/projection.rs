//! Import local proved scalar bounds before discarding relational history.
//!
//! This is a numerical overapproximation, with no native SMT/model extraction.
//! Summary import memoizes numerical upper bounds; branch metadata keeps its
//! feasibility checks and precision diagnostics. Both share query accounting.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    AlgebraRef, EvmByteArray, EvmMachinePayload, EvmValue, MachineError, RelationState, WorkBudget,
};
use crate::domain::{
    relational::{EvmScalarQuery, QueryReason, RelationLimits},
    symbolic::{ExprId, ExprKind},
};
use crate::resource::clone_payment::{checkpoint, finish_quote, pay};
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

fn numeric_equal<D: EvmNumericAlgebra>(
    left: &D::Element,
    right: &D::Element,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<bool, MachineError> {
    checkpoint(budget)?;
    pay(
        budget,
        domain.numeric_algebra().summary_equal_work(left, right),
    )?;
    let equal = left == right;
    checkpoint(budget)?;
    Ok(equal)
}

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
        match pay(self.budget, cost) {
            Ok(()) => true,
            Err(error) => {
                self.outcome = Err(error.into());
                false
            }
        }
    }

    /// Callers stop before another segment could materialize an owned copy.
    pub(in crate::analysis) fn stopped(&self) -> bool {
        !matches!(self.outcome, Ok(true))
    }

    fn expression(&mut self, expression: &ExprId, value: &mut EvmValue<D::Element>) {
        // A retained literal is already a proof about this source. Numeric
        // singleton discovery is a different, separately priced operation.
        if self.stopped() || expression.as_constant().is_some() || self.relations.is_empty() {
            return;
        }
        if let Err(error) = checkpoint(self.budget) {
            self.outcome = Err(error.into());
            return;
        }
        let summary = matches!(&self.policy, Policy::Summary);
        if summary && let Some(cached) = self.cache.get(expression) {
            for (old, refined) in cached {
                // Geometry/copy reservations do not fund an opaque native Eq.
                // Preserve short-circuiting, but pay each predicate actually run.
                let matches = (|| {
                    if numeric_equal(old, value.numeric(), self.domain, self.budget)? {
                        return Ok(true);
                    }
                    match refined {
                        Some(refined) => {
                            numeric_equal(refined, value.numeric(), self.domain, self.budget)
                        }
                        None => Ok(false),
                    }
                })();
                match matches {
                    Err(error) => {
                        self.outcome = Err(error);
                        return;
                    }
                    Ok(false) => continue,
                    Ok(true) => {}
                }
                if let Some(refined) = refined {
                    if let Err(error) = pay(self.budget, refined.clone_work()) {
                        self.outcome = Err(error.into());
                        return;
                    }
                    value.replace_numeric_after_singleton(refined.clone(), None);
                }
                return;
            }
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
        let answer = match self.relations.refine_numeric_budgeted(
            expression,
            value,
            self.domain,
            self.limits,
            self.budget,
        ) {
            Ok(answer) => answer,
            Err(_) => {
                self.outcome = Err(checkpoint(self.budget)
                    .err()
                    .map_or(MachineError::Work, MachineError::from));
                return;
            }
        };
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
        if summary {
            // One entry also recognizes its refined bound on the next visit.
            // Quote every unavoidable native copy before invoking Clone.
            if let Err(error) = checkpoint(self.budget) {
                self.outcome = Err(error.into());
                return;
            }
            let copies = (|| {
                let old = finish_quote(value.numeric().clone_work(), self.budget)?;
                let refined = match &refined {
                    Some(numeric) => finish_quote(numeric.clone_work(), self.budget)?,
                    None => 0,
                };
                finish_quote(old.saturating_add(refined), self.budget)
            })();
            let copies = match copies {
                Ok(copies) => copies,
                Err(error) => {
                    self.outcome = Err(error.into());
                    return;
                }
            };
            if !self.reserve(copies) {
                return;
            }
            self.cache
                .entry(expression.clone())
                .or_default()
                .push((value.numeric().clone(), refined.clone()));
        }
        if let Some(refined) = refined {
            // Native refinement already owns this bound and its discovery.
            value.replace_numeric_after_singleton(refined, None);
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
        if self.stopped() || self.relations.is_empty() {
            return;
        }
        // Mutable imbl traversal can clone shared map nodes before reaching
        // our callback. The owned-Store quote bounds those native copies too.
        #[cfg(feature = "imbl")]
        if let Err(error) = store.pay_clone_with(self.budget) {
            self.outcome = Err(error.into());
            return;
        }
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
        // No scalar can change without a retained guarantee. In particular,
        // do not materialize every rollback Cow merely to run no-op callbacks.
        if self.relations.is_empty() {
            return checkpoint(self.budget).map_err(MachineError::from);
        }
        self.store(&mut payload.store);
        self.outcome.clone()?;
        payload.call_stack.transform(&mut |part| {
            match part {
                StatePart::Value(value) => self.value(value),
                StatePart::Bytes(bytes) => self.bytes(bytes),
                StatePart::Store(store) => {
                    if matches!(store, std::borrow::Cow::Borrowed(_)) {
                        store.pay_clone_with(self.budget)?;
                    }
                    self.store(store.to_mut());
                }
            }
            self.outcome.clone().map(|_| ())
        })
    }
}
