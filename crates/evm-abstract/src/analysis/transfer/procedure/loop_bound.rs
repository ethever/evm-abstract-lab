//! Header-local numerical invariants. Executable identities are never the
//! convergence criterion; independent roots are built only after a bound grows.

use super::{
    Domain, Entry, FrameCode, MachineError, MachinePayload, PayloadSummary, WorkBudget,
    bounded::{BoundedState, erase_guarantees, reserve},
    form, join, semantic_eq,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(in crate::analysis) struct LoopBound {
    representation: BoundedState,
    bound: PayloadSummary,
}

fn project(
    payload: &MachinePayload,
    entry: &Entry,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Result<PayloadSummary, MachineError> {
    if payload
        .call_stack
        .iter()
        .any(|frame| frame.code == FrameCode::Unknown)
    {
        return Err(MachineError::Shape);
    }
    reserve(
        budget,
        payload
            .work_size()
            .saturating_mul(4)
            .saturating_add(super::payload::alias_form_work(payload))
            .saturating_add(super::storage_observations::form_work(payload, domain)),
    )?;
    // These are optional guarantees, not coordinates of the loop lattice.
    // Erase them consistently before comparing or independently replaying roots.
    erase_guarantees(form(payload, entry, domain)?)
}

impl LoopBound {
    pub(in crate::analysis) fn new(
        old: &MachinePayload,
        incoming: &MachinePayload,
        entry: &Entry,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<(Self, MachinePayload), MachineError> {
        if old.key() != incoming.key() {
            return Err(MachineError::Shape);
        }
        let old = project(old, entry, domain, budget)?;
        let incoming = project(incoming, entry, domain, budget)?;
        reserve(
            budget,
            old.work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let layout = join(&old, &incoming, domain, false)?;
        let representation = BoundedState::freeze(&layout, false, domain)?;
        let bound = join(&old, &incoming, domain, true)?;
        let (representation, bound, payload) = representation.materialize(bound, domain, budget)?;
        Ok((
            Self {
                representation,
                bound,
            },
            payload,
        ))
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        self.representation
            .work_size()
            .saturating_add(self.bound.work_size())
    }

    pub(in crate::analysis) fn update(
        &self,
        incoming: &MachinePayload,
        entry: &Entry,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<Option<(Self, MachinePayload)>, MachineError> {
        if incoming.key() != *self.representation.location() {
            return Err(MachineError::Shape);
        }
        let incoming = project(incoming, entry, domain, budget)?;
        let joined = self
            .representation
            .join(&self.bound, &incoming, domain, budget, false)?;
        if semantic_eq(&joined, &self.bound) {
            MachineError::check_cancelled()?;
            return Ok(None);
        }
        let bound = join(&self.bound, &joined, domain, true)?;
        let (representation, bound, payload) = self
            .representation
            .clone()
            .materialize(bound, domain, budget)?;
        Ok(Some((
            Self {
                representation,
                bound,
            },
            payload,
        )))
    }
}
