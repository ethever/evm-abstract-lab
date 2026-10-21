//! Executable zero/nonzero decisions on one unchanged numeric operand.
//!
//! A JUMPI condition is an arbitrary word, unlike a Boolean range predicate.
//! A negative singleton answer therefore keeps the nonzero edge possible;
//! separately paid zero membership can still exclude its fallthrough edge.

use super::{candidates, query};
use crate::{
    U256,
    analysis::FrontierReason,
    domain::{EvmValue, SingletonProof, WordElement},
    resource::WorkBudget,
};

#[cfg(test)]
mod tests;

pub(super) struct Truth {
    pub(super) zero: bool,
    pub(super) nonzero: bool,
    pub(super) singleton: SingletonProof,
}

impl Truth {
    pub(super) fn prepare<N: WordElement>(
        value: &EvmValue<N>,
        proof: SingletonProof,
        budget: &mut WorkBudget,
    ) -> Result<Self, FrontierReason> {
        let (cover, singleton) = candidates::prepare_with_proof(value, proof, budget)?;
        if let SingletonProof::Answer(Some(word)) = singleton {
            return Ok(Self {
                zero: word.is_zero(),
                nonzero: !word.is_zero(),
                singleton,
            });
        }
        // A complete cover excluding zero settles membership without invoking
        // opaque code. An open value's negative query is reused by both edges.
        let may_contain_zero = cover
            .as_ref()
            .is_none_or(|cover| cover.contains(&U256::ZERO));
        let nonzero = cover
            .as_ref()
            .is_none_or(|cover| cover.iter().any(|word| !word.is_zero()));
        let zero = if may_contain_zero {
            query::contains(value.numeric(), U256::ZERO, budget)?
        } else {
            false
        };
        Ok(Self {
            zero,
            nonzero,
            singleton,
        })
    }
}

/// EIP-1052 distinguishes an empty account from an existing nonempty account
/// with empty code. Code hashing does not fund balance or nonce discovery.
pub(super) fn code_hash<D: crate::domain::EvmNumericAlgebra>(
    store: &crate::world::EvmStore<D::Element>,
    target: alloy_primitives::Address,
    context: &mut super::TransferContext<'_, D>,
) -> Result<EvmValue<D::Element>, FrontierReason> {
    query::checkpoint(context.budget)?;
    let Some(bytes) = super::code_bytes(store, target) else {
        return Ok(EvmValue::top());
    };
    if bytes.is_empty() && store.existence(target) == crate::world::Existence::Absent {
        return Ok(EvmValue::constant(U256::ZERO));
    }
    let hash = EvmValue::constant(U256::from_be_slice(
        alloy_primitives::keccak256(&bytes).as_slice(),
    ));
    if !bytes.is_empty() {
        return Ok(hash);
    }
    let balance = store.read_balance(target);
    let balance = Truth::prepare(&balance, SingletonProof::NotQueried, context.budget)?;
    if !balance.zero {
        return Ok(hash);
    }
    let nonce = store.nonce(target);
    let nonce = Truth::prepare(&nonce, SingletonProof::NotQueried, context.budget)?;
    let value = if nonce.zero && (balance.nonzero || nonce.nonzero) {
        context.domain.join(&hash, &EvmValue::constant(U256::ZERO))
    } else if nonce.zero {
        EvmValue::constant(U256::ZERO)
    } else {
        hash
    };
    query::checkpoint(context.budget)?;
    Ok(value)
}
