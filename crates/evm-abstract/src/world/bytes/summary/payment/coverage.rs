//! Borrowed sufficient proof; incompatible content keeps the original join.

use super::{CloneError, Content, EvmByteArraySummary, WorkBudget};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::clone_payment::checkpoint,
};

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Prove coverage using identical content, a covering length and every
    /// retained word. False includes unavailable proof work and unsupported
    /// representations. No native join, projection or new identity is built.
    pub(crate) fn covers_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CloneError> {
        let entries = self
            .clone_prefix_work()
            .saturating_add(other.clone_prefix_work());
        let levels = (usize::BITS - entries.max(1).leading_zeros()) as usize;
        if !summary::try_proof_work(
            budget,
            entries
                .saturating_add(1)
                .saturating_mul(32usize.saturating_add(levels.saturating_mul(16))),
        )? || (self.memory && !other.memory)
        {
            return Ok(false);
        }
        match (&self.content, &other.content) {
            (Content::Zero, Content::Zero) => {}
            (Content::Exact(left), Content::Exact(right)) => {
                if !summary::try_proof_work(budget, left.len().min(right.len()).saturating_add(1))?
                    || left != right
                {
                    return Ok(false);
                }
            }
            (Content::Cells(left), Content::Cells(right)) if left.len() == right.len() => {
                for ((index, value), (other_index, other_value)) in left.iter().zip(right) {
                    if index != other_index
                        || !summary::equal_scalar(value, other_value, domain, budget)?
                    {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
        if !summary::covers_scalar(&self.length, &other.length, domain, budget)? {
            return Ok(false);
        }
        // words::join intersects keys. An omitted incoming key denotes no word
        // guarantee; extra incoming keys cannot invalidate our weaker bound.
        for (index, value) in &self.words {
            let Some(incoming) = other.words.get(index) else {
                return Ok(false);
            };
            if !summary::covers_scalar(value, incoming, domain, budget)? {
                return Ok(false);
            }
        }
        checkpoint(budget)?;
        Ok(true)
    }
}
