//! Actual procedure callers pay export, replay, comparison and static effects.
//!
//! Fresh replay roots retain their actual native operands beside observational
//! bindings. Neither observation export nor symbolic identity replaces N.

use super::metered::{code_comparison_work, pay_structure};
use super::*;
use crate::{
    domain::{WordObservation, summary as scalar},
    resource::{
        CloneError, SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};

mod equality;
mod projection;
mod replay;
#[cfg(test)]
mod tests;

fn source(error: impl Into<StoreError>) -> SummaryError<StoreError> {
    SummaryError::Source(error.into())
}

/// The element's retention contract explicitly bounds one observation export.
/// This payment does not authorize a later import, copy or numeric query.
fn observation<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<WordObservation, CloneError> {
    pay(budget, value.numeric().work_size())?;
    let out = value.numeric().observation();
    checkpoint(budget)?;
    Ok(out)
}

fn clone_map<K: Ord + Clone, N: WordElement>(
    values: &OrderedMap<K, EvmValue<N>>,
    budget: &mut WorkBudget,
) -> Result<OrderedMap<K, EvmValue<N>>, CloneError> {
    pay_structure(values.len(), budget)?;
    let mut out = OrderedMap::new();
    for (key, value) in values {
        out.insert(key.clone(), scalar::clone_value(value, budget)?);
    }
    checkpoint(budget)?;
    Ok(out)
}

impl<N: WordElement> EvmStoreSummary<N> {
    /// Static delivery changes only monotone touches. The executable snapshot
    /// copy is funded by the runtime Store's original native clone invoice.
    pub(crate) fn apply_static_touches_with_payment(
        &self,
        input: &EvmStore<N>,
        budget: &mut WorkBudget,
    ) -> Result<EvmStore<N>, CloneError> {
        pay_structure(
            input.touched.len().saturating_add(self.touched.len()),
            budget,
        )?;
        input.pay_clone_with(budget)?;
        let mut out = input.clone();
        checkpoint(budget)?;
        (out.touched, out.touched_default) = sparse::combine(
            (&input.touched, &input.touched_default),
            (&self.touched, &self.touched_default),
            |left, right| lifecycle::or(*left, *right),
        );
        checkpoint(budget)?;
        Ok(out)
    }
}
