//! Import local proved scalar bounds before discarding relational history.
//!
//! This is a numerical overapproximation, with no native SMT/model extraction.
//! Each original term/bound pair is reduced once per projection; unsupported
//! inverse constraints keep their old upper bound rather than guessing a value.

use super::{
    AbstractValue, ByteArray, Domain, MachineError, MachinePayload, NumericValue, RelationState,
    Snapshot, Store, WorkBudget,
};
use crate::domain::{
    relational::{QueryReason, RelationLimits, ScalarQuery},
    symbolic::ExprId,
};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

pub(super) struct Projection<'a> {
    relations: &'a RelationState,
    relations_work_size: Option<usize>,
    #[cfg(test)]
    relations_work_size_scans: usize,
    domain: Domain,
    limits: &'a RelationLimits,
    budget: &'a mut WorkBudget,
    cache: BTreeMap<ExprId, Vec<(NumericValue, Option<NumericValue>)>>,
    failure: Option<MachineError>,
}

impl<'a> Projection<'a> {
    pub(super) fn new(
        relations: &'a RelationState,
        domain: Domain,
        limits: &'a RelationLimits,
        budget: &'a mut WorkBudget,
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
            failure: None,
        }
    }

    fn expression(&mut self, expression: &ExprId, value: &mut AbstractValue) {
        if self.failure.is_some() || value.singleton().is_some() || self.relations.is_empty() {
            return;
        }
        if let Some(cached) = self.cache.get(expression)
            && let Some((_, refined)) = cached.iter().find(|(old, _)| old == value.numeric())
        {
            if let Some(refined) = refined {
                *value = value.clone().with_numeric(refined.clone());
            }
            return;
        }
        if !self.budget.charge(expression.work_size().saturating_add(
            *self.relations_work_size.get_or_insert_with(|| {
                #[cfg(test)]
                {
                    self.relations_work_size_scans += 1;
                }
                self.relations.work_size()
            }),
        )) {
            self.failure = Some(MachineError::work_failure());
            return;
        }
        if !self.relations.has_direct_numeric_guarantee(expression) {
            return;
        }
        if !self
            .budget
            .charge(self.domain.refinement_work(value, self.relations.len()))
        {
            self.failure = Some(MachineError::work_failure());
            return;
        }
        let old = value.numeric().clone();
        let refined =
            match self
                .relations
                .refine_numeric(expression, &old, self.domain, self.limits)
            {
                ScalarQuery::Refined { numeric, .. } => Some(*numeric),
                ScalarQuery::Unknown(QueryReason::Cancelled) => {
                    self.failure = Some(MachineError::Relations(QueryReason::Cancelled));
                    return;
                }
                // The canonical form deliberately forgets relations. Keeping the
                // old upper bound is also sound for unsupported or contradictory
                // local projections; no infeasible path is used as a base case.
                ScalarQuery::Infeasible | ScalarQuery::Unchanged | ScalarQuery::Unknown(_) => None,
            };
        if let Some(refined) = &refined {
            *value = value.clone().with_numeric(refined.clone());
            if refined != &old {
                self.cache
                    .entry(expression.clone())
                    .or_default()
                    .push((refined.clone(), Some(refined.clone())));
            }
        }
        self.cache
            .entry(expression.clone())
            .or_default()
            .push((old, refined));
    }

    fn value(&mut self, value: &mut AbstractValue) {
        if let Some(expression) = value.expression().cloned() {
            self.expression(&expression, value);
        }
    }

    pub(super) fn bytes(&mut self, bytes: &mut ByteArray) {
        bytes.update_values(&mut |value| self.value(value));
        bytes.refine_operands(&mut |expression, value| self.expression(expression, value));
    }

    pub(super) fn store(&mut self, store: &mut Store) {
        store.update_values(&mut |value| self.value(value));
        store.refine_operands(&mut |expression, value| self.expression(expression, value));
    }

    pub(super) fn finish(self) -> Result<(), MachineError> {
        self.failure.map_or(Ok(()), Err)
    }

    pub(super) fn payload(&mut self, payload: &mut MachinePayload) {
        self.store(&mut payload.store);
        for frame in payload.call_stack.iter_mut() {
            for value in &mut frame.stack {
                self.value(value);
            }
            self.value(&mut frame.call_value);
            for value in [&mut frame.address_operand, &mut frame.caller_operand]
                .into_iter()
                .flatten()
            {
                self.value(value);
            }
            for bytes in [
                &mut frame.memory,
                &mut frame.calldata,
                &mut frame.returndata,
            ] {
                self.bytes(bytes);
            }
            let mut saved = frame.saved_store.state().clone();
            self.store(&mut saved);
            frame.saved_store = Snapshot::from_state(saved);
        }
        for child in payload.call_stack.children_mut() {
            self.value(&mut child.continuation.output_offset);
            self.value(&mut child.continuation.output_size);
        }
    }
}
