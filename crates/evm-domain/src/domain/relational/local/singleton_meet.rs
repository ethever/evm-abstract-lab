//! Direct intersection with an already canonical word under an explicit proof.
//!
//! A semantic singleton alone is insufficient: bounded generic reduction may
//! retain noncanonical components. With a canonical operand, each successful
//! component meet is already exact; reduction cannot strengthen its numeric
//! result. The certified product preserves this one-element value at every valid
//! capacity, round/fact limit and relation policy. Unsupported shapes decline
//! before the caller reserves and executes the original generic intersection.

use super::{LocalResult, Stop, Work};
use crate::domain::{AlgebraRef, EvmNumericAlgebra, WordElement};
use alloy_primitives::U256;

// Prices bound performed fixed-width actions in limb units. No candidate set
// traversal or allocation is needed to quote its stored cardinality.
const WORD_LIMBS: usize = 4;
const CANONICAL_WORK: usize = 32 * WORD_LIMBS;
const ADMISSION_WORK: usize = 4 * WORD_LIMBS;
const MEMBERSHIP_WORK: usize = 24 * WORD_LIMBS;
const CLONE_WORK: usize = 8 * WORD_LIMBS;

pub(super) fn meet<D: EvmNumericAlgebra>(
    left: &D::Element,
    right: &D::Element,
    domain: AlgebraRef<'_, D>,
    work: &mut Work<'_>,
) -> LocalResult<Option<D::Element>> {
    if !domain.numeric_algebra().supports_product_scalar_shortcuts() {
        return Ok(None);
    }
    for (canonical, other) in [(left, right), (right, left)] {
        let Some(value) = canonical_word(canonical, work)? else {
            continue;
        };
        work.prepay(ADMISSION_WORK)?;
        if other
            .congruence()
            .modulus_residue()
            .is_some_and(|(modulus, _)| modulus != U256::ONE)
        {
            // General modulo membership performs division. Keep its existing
            // generic reservation rather than pricing it as a limb comparison.
            return Ok(None);
        }
        let candidates = other.finite_constants().cardinality().unwrap_or(0);
        // A BTreeSet lookup cannot compare more than its retained N keys. This
        // conservative input-sized bound avoids depending on private tree arity.
        work.prepay(MEMBERSHIP_WORK.saturating_add(candidates.saturating_mul(WORD_LIMBS)))?;
        if !other.contains(value) {
            return Err(Stop::Bottom);
        }
        // No reduction or candidate rebuilding occurs here.
        // The clone shares the canonical singleton's immutable finite storage.
        work.prepay(CLONE_WORK)?;
        return Ok(Some(canonical.clone()));
    }
    Ok(None)
}

fn canonical_word<N: WordElement>(value: &N, work: &mut Work<'_>) -> LocalResult<Option<U256>> {
    work.prepay(1)?;
    if value.finite_constants().cardinality() != Some(1) {
        return Ok(None);
    }
    // Covers singleton extraction, component singleton queries/comparisons, and
    // singleton() checking membership against every component of this operand.
    work.prepay(CANONICAL_WORK)?;
    let word = value
        .finite_constants()
        .singleton()
        .expect("cardinality established a finite singleton");
    // Valid bit masks and normalized intervals have the exact component
    // representation when their singleton is this word, including signed bounds.
    if value.known_bits().singleton() != Some(word)
        || value.interval().singleton() != Some(word)
        || value.congruence().singleton() != Some(word)
        || value.observation().nonzero() != (word != U256::ZERO)
    {
        return Ok(None);
    }
    Ok(value.singleton())
}

#[cfg(test)]
mod tests;
