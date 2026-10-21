//! Price complete owned snapshots before creating any of their clones.

use super::{EvmMachinePayload, FrontierReason, MachineError};
use crate::{
    domain::{
        WordElement,
        relational::{Constraint, QueryReason},
    },
    resource::{
        CloneError, WorkBudget,
        clone_payment::{finish_quote, pay},
    },
};

impl From<CloneError> for MachineError {
    fn from(error: CloneError) -> Self {
        match error {
            CloneError::Cancelled => Self::Relations(QueryReason::Cancelled),
            CloneError::Work => Self::Work,
        }
    }
}

impl From<CloneError> for FrontierReason {
    fn from(error: CloneError) -> Self {
        MachineError::from(error).frontier_reason()
    }
}

impl<N: WordElement> EvmMachinePayload<N> {
    fn relation_clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        let constraints = self.relations.constraints();
        pay(budget, constraints.len().saturating_add(1))?;
        // RelationPath is an Arc. Only Members owns another variable container;
        // its cached length prices those U256 entries without walking them.
        let mut work = 8usize.saturating_add(constraints.len().saturating_mul(20));
        for constraint in constraints {
            if let Constraint::Members { values, .. } = constraint {
                work = work.saturating_add(values.len().saturating_mul(6));
            }
        }
        finish_quote(work, budget)
    }

    pub(crate) fn clone_work_with(&self, budget: &mut WorkBudget) -> Result<usize, CloneError> {
        let work = self
            .relation_clone_work_with(budget)?
            .saturating_add(self.call_stack.clone_work_with(budget)?)
            .saturating_add(self.store.clone_work_with(budget)?);
        finish_quote(work, budget)
    }

    pub(crate) fn pay_clone_with(&self, budget: &mut WorkBudget) -> Result<(), CloneError> {
        let work = self.relation_clone_work_with(budget)?;
        self.call_stack.pay_clone_with(budget)?;
        self.store.pay_clone_with(budget)?;
        pay(budget, work)
    }
}

#[cfg(test)]
mod tests;
