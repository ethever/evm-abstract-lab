//! Exact summary equality prices every actually compared native value.

use super::{CloneError, EvmByteArraySummary, WorkBudget};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::clone_payment::{checkpoint, pay},
};

impl<N: WordElement> EvmByteArraySummary<N> {
    pub(crate) fn eq_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        pay(budget, 4)?;
        if !summary::equal(&self.length, &other.length, domain, budget)?
            || self.memory != other.memory
            || !self
                .content
                .equal_with_payment(&other.content, domain, budget)?
            || self.words.len() != other.words.len()
        {
            return Ok(false);
        }
        pay(budget, self.words.len().saturating_mul(2).saturating_add(1))?;
        for ((index, value), (other_index, other_value)) in self.words.iter().zip(&other.words) {
            if index != other_index || !summary::equal(value, other_value, domain, budget)? {
                return Ok(false);
            }
        }
        checkpoint(budget)?;
        Ok(true)
    }
}
