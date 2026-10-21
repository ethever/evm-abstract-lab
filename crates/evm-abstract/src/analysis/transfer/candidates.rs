//! Complete word covers used by executable target and extent dispatch.
//!
//! A domain can know one exact word without retaining finite membership.
//! Preserve that knowledge without allocating a temporary candidate set.

use crate::{
    U256,
    analysis::FrontierReason,
    domain::{EvmValue, SingletonProof, WordElement},
    resource::WorkBudget,
    world::{AddressInput, EvmEntry},
};
use std::collections::BTreeSet;

pub(super) struct Candidates<'a> {
    finite: Option<&'a BTreeSet<U256>>,
    singleton: Option<U256>,
}

impl Candidates<'_> {
    pub(super) fn single(word: U256) -> Self {
        Self {
            finite: None,
            singleton: Some(word),
        }
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = &U256> {
        self.finite
            .into_iter()
            .flatten()
            .chain(self.singleton.iter())
    }

    pub(super) fn len(&self) -> usize {
        self.finite.map_or(1, BTreeSet::len)
    }

    pub(super) fn contains(&self, word: &U256) -> bool {
        self.finite.map_or_else(
            || self.singleton.as_ref() == Some(word),
            |set| set.contains(word),
        )
    }
}

/// Counts need no numeric discovery: both an opaque singleton and an unknown
/// value contribute one branch. The finite view has cached cardinality.
pub(super) fn count<N: WordElement>(value: &EvmValue<N>) -> usize {
    value.constants().map_or(1, BTreeSet::len)
}

/// A pure, caller-funded cover. Production quotations use `prepare` and reuse
/// its answer rather than executing the fallback before reserving query work.
#[cfg(test)]
pub(super) fn complete<N: WordElement>(value: &EvmValue<N>) -> Option<Candidates<'_>> {
    if let Some(finite) = value.constants() {
        return Some(Candidates {
            finite: Some(finite),
            singleton: None,
        });
    }
    value.singleton().map(Candidates::single)
}

pub(super) fn prepare<'a, N: WordElement>(
    value: &'a EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<(Option<Candidates<'a>>, SingletonProof), FrontierReason> {
    prepare_with_proof(value, SingletonProof::NotQueried, budget)
}

/// A complete operation may already have discovered its actual result. Reuse
/// that proof only for this unchanged numeric element, including a negative
/// answer. Cached finite covers and trusted literal terms need no discovery.
pub(super) fn prepare_with_proof<'a, N: WordElement>(
    value: &'a EvmValue<N>,
    mut proof: SingletonProof,
    budget: &mut WorkBudget,
) -> Result<(Option<Candidates<'a>>, SingletonProof), FrontierReason> {
    super::query::checkpoint(budget)?;
    if let SingletonProof::Answer(Some(word)) = proof {
        return Ok((Some(Candidates::single(word)), proof));
    }
    if let Some(word) = value
        .expression()
        .and_then(crate::domain::symbolic::ExprId::as_constant)
    {
        return Ok((
            Some(Candidates::single(word)),
            SingletonProof::Answer(Some(word)),
        ));
    }
    if let Some(finite) = value.constants() {
        let proof = if finite.len() == 1 {
            SingletonProof::Answer(finite.first().copied())
        } else {
            proof
        };
        return Ok((
            Some(Candidates {
                finite: Some(finite),
                singleton: None,
            }),
            proof,
        ));
    }
    let answer = super::query::resolve(value.numeric(), &mut proof, budget)?;
    Ok((
        answer.map(Candidates::single),
        SingletonProof::Answer(answer),
    ))
}

/// Compare scoped input identities without constructing an environment value.
/// The symbolic owner already supplies its concrete code-storage address.
pub(super) fn is_self<N: WordElement>(value: &EvmValue<N>, entry: &EvmEntry<N>) -> bool {
    let AddressInput::Symbolic(symbol) = entry.environment.to else {
        return false;
    };
    value.identity().input().is_some_and(|input| {
        input.symbol() == symbol && Some(input.scope()) == entry.environment.input_scope.id()
    })
}
