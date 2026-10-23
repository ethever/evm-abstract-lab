//! Every newly joined, widened and projected carrier owns its later tariffs.

use super::{CloneError, Content, EvmByteArraySummary, WorkBudget, coordinate_work, numeric_equal};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::clone_payment::{checkpoint, pay},
};
use std::collections::BTreeMap;

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Same content union, widening and canonicalization as the pure join.
    pub(crate) fn join_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay(budget, 8)?;
        let content = if self
            .content
            .equal_with_payment(&other.content, domain, budget)?
        {
            self.content.clone_with_payment(budget)?
        } else {
            let mut keys = self.content.keys_with_payment(budget)?;
            let other_keys = other.content.keys_with_payment(budget)?;
            let count = keys.len().saturating_add(other_keys.len());
            let sort = count.saturating_mul(coordinate_work(count).saturating_add(4));
            pay(budget, sort)?;
            keys.extend(other_keys);
            keys.sort_unstable();
            keys.dedup();
            let mut cells = BTreeMap::new();
            for index in keys.into_iter().take(super::super::COPY_FACT_LIMIT) {
                let left = self.content.at_with_payment(index, domain, budget)?;
                let right = other.content.at_with_payment(index, domain, budget)?;
                let value = summary::join_scalar(&left, &right, domain, widen, budget)?;
                let unknown = summary::unknown_byte(domain, budget)?;
                if !numeric_equal(&value, &unknown, domain, budget)? {
                    pay(budget, coordinate_work(cells.len()).saturating_add(2))?;
                    cells.insert(index, value);
                }
            }
            Content::Cells(cells)
        };
        let length = summary::join_scalar(&self.length, &other.length, domain, widen, budget)?;
        pay(budget, self.words.len().saturating_add(1))?;
        let mut words = BTreeMap::new();
        for (index, value) in &self.words {
            pay(budget, coordinate_work(other.words.len()))?;
            let Some(other_value) = other.words.get(index) else {
                continue;
            };
            let value = summary::join_scalar(value, other_value, domain, widen, budget)?;
            if !summary::is_top(&value, domain, budget)? {
                pay(budget, coordinate_work(words.len()).saturating_add(2))?;
                words.insert(*index, value);
            }
        }
        checkpoint(budget)?;
        Ok(Self {
            length,
            memory: self.memory && other.memory,
            content,
            words,
        })
    }
}
