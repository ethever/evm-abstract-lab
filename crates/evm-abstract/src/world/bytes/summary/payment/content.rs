//! Structural content decisions never imply permission for native predicates.

use super::{CloneError, Content, EvmValue, WorkBudget, coordinate_work};
use crate::{
    U256,
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::clone_payment::{checkpoint, pay},
};
use std::{collections::BTreeMap, sync::Arc};

impl<N: WordElement> Content<N> {
    fn key_count(&self) -> usize {
        match self {
            Self::Zero => 0,
            Self::Exact(bytes) => bytes.len().min(super::super::COPY_FACT_LIMIT),
            Self::Cells(cells) => cells.len(),
        }
    }

    pub(in super::super) fn keys_with_payment(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Vec<usize>, CloneError> {
        pay(budget, self.key_count().saturating_add(1).saturating_mul(2))?;
        let keys = self.keys();
        checkpoint(budget)?;
        Ok(keys)
    }

    pub(super) fn equal_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        pay(budget, 1)?;
        match (self, other) {
            (Self::Zero, Self::Zero) => Ok(true),
            (Self::Exact(left), Self::Exact(right)) => {
                pay(budget, left.len().min(right.len()).saturating_add(1))?;
                let equal = left == right;
                checkpoint(budget)?;
                Ok(equal)
            }
            (Self::Cells(left), Self::Cells(right)) => {
                if left.len() != right.len() {
                    return Ok(false);
                }
                pay(budget, left.len().saturating_mul(2).saturating_add(1))?;
                for ((index, value), (other_index, other_value)) in left.iter().zip(right) {
                    if index != other_index || !summary::equal(value, other_value, domain, budget)?
                    {
                        return Ok(false);
                    }
                }
                checkpoint(budget)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn clone_with_payment(&self, budget: &mut WorkBudget) -> Result<Self, CloneError> {
        pay(budget, self.key_count().saturating_add(2))?;
        let content = match self {
            Self::Zero => Self::Zero,
            Self::Exact(bytes) => Self::Exact(Arc::clone(bytes)),
            Self::Cells(cells) => {
                let mut copied = BTreeMap::new();
                for (index, value) in cells {
                    let value = summary::clone_value(value, budget)?;
                    pay(budget, coordinate_work(copied.len()).saturating_add(2))?;
                    copied.insert(*index, value);
                }
                Self::Cells(copied)
            }
        };
        checkpoint(budget)?;
        Ok(content)
    }

    pub(in super::super) fn at_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        index: usize,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmValue<N>, CloneError> {
        pay(budget, 1)?;
        let constant = match self {
            Self::Zero => U256::ZERO,
            Self::Exact(bytes) => U256::from(bytes.get(index).copied().unwrap_or(0)),
            Self::Cells(cells) => {
                pay(budget, coordinate_work(cells.len()))?;
                return match cells.get(&index) {
                    Some(value) => summary::clone_value(value, budget),
                    None => summary::unknown_byte(domain, budget),
                };
            }
        };
        // The summary factory preserves Content::at's unknown provenance while
        // using the selected domain's literal construction/release tariff.
        summary::constant(constant, domain, budget)
    }
}
