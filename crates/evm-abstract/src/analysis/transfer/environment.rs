//! Budgeted immutable environment queries.

use super::{candidates, query};
use crate::{
    U256,
    analysis::FrontierReason,
    domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement, symbolic::ExprId},
    resource::WorkBudget,
    world::{EvmBlobHashes, EvmEnvironmentValues},
};

/// The instruction's existing envelope owns output construction and joins.
/// Its environment/input discoveries are independent and paid here exactly once.
pub(super) fn block_hash<D: EvmNumericAlgebra>(
    environment: &EvmEnvironmentValues<D::Element>,
    index: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, FrontierReason> {
    let (current, indices) = prepare(&environment.number, index, budget)?;
    let value = environment.block_hash_with(
        current,
        indices.as_ref().map(|indices| indices.iter().copied()),
        domain,
    );
    query::checkpoint(budget)?;
    Ok(value)
}

pub(super) fn blob_hash<D: EvmNumericAlgebra>(
    blobs: &EvmBlobHashes<D::Element>,
    index: &EvmValue<D::Element>,
    scope: Option<u64>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<D::Element>, FrontierReason> {
    let (length, indices) = prepare(&blobs.length, index, budget)?;
    let value = blobs.get_with(
        length,
        indices.as_ref().map(|indices| indices.iter().copied()),
        domain,
        scope,
    );
    query::checkpoint(budget)?;
    Ok(value)
}

fn prepare<'a, N: WordElement>(
    bound: &EvmValue<N>,
    index: &'a EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<(Option<U256>, Option<candidates::Candidates<'a>>), FrontierReason> {
    query::checkpoint(budget)?;
    // A cached MAX index is always out of range, even without NUMBER. Keep the
    // prepared cover for dispatch instead of discovering the same input again.
    let cached = if index.constants().is_some()
        || index.expression().and_then(ExprId::as_constant).is_some()
    {
        Some(candidates::prepare(index, budget)?.0)
    } else {
        None
    };
    let current = if cached
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|indices| indices.len() == 1 && indices.contains(&U256::MAX))
    {
        None
    } else {
        let (_, mut proof) = candidates::prepare(bound, budget)?;
        query::resolve(bound.numeric(), &mut proof, budget)?
    };
    // Genesis/an empty blob list excludes every index without opaque discovery.
    let indices = if current == Some(U256::ZERO) {
        None
    } else {
        match cached {
            Some(indices) => indices,
            None => candidates::prepare(index, budget)?.0,
        }
    };
    Ok((current, indices))
}

#[cfg(test)]
mod tests;
