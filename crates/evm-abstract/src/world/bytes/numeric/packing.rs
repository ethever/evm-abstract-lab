//! Exact replacements for the numeric word reader's 32 SHLs and 31 ORs.

use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{EvmValue, provenance::Origin};
use crate::world::bytes::conditional::Context;

/// Reconstruct only cases equivalent to the scalar fold, including metadata.
/// The caller still owns the original word expression and its byte-state root.
pub(super) fn pack<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    bytes: &[EvmValue<N>],
    domain: AlgebraRef<'_, D>,
    context: &Context<'_, N, D>,
) -> Option<EvmValue<N>> {
    let bytes: &[EvmValue<N>; 32] = bytes.try_into().ok()?;
    let numeric =
        context.pack_word_numeric(domain, &std::array::from_fn(|i| bytes[i].numeric()))?;

    // Every SHL adds Constant (its shift) and Arithmetic. OR unions all byte
    // sources. Construct a fresh value: neither input/copy identity, expression
    // nor the code-address role survives these arithmetic operations.
    let mut value = EvmValue::<N>::from_numeric(numeric);
    // The pack fee funds the numeric hook, not a query on its opaque result.
    // One paid proof serves both metadata decisions; a refused/cancelled proof
    // must stop before assembling metadata or entering the scalar fallback.
    let singleton = context.prove_singleton(&value);
    if context.stopped() || (domain.spec().relations().enabled && singleton.is_none()) {
        return None;
    }
    if bytes
        .iter()
        .all(|byte| byte.provenance().origins().sources().is_some())
    {
        value.set_origin(Origin::Constant);
        value = value.with_origin(Origin::Arithmetic);
        for byte in bytes {
            for origin in byte
                .provenance()
                .origins()
                .sources()
                .expect("checked sources")
            {
                value = value.with_origin(*origin);
            }
        }
    }
    // apply propagates this marker even with relations disabled, but every
    // concrete SHL clears it. Do not use project, which would clear it on Top.
    if singleton.is_none() && bytes.iter().any(EvmValue::<N>::symbolic_limit_reached) {
        value = value.with_symbolic_limit();
    }
    (!context.stopped()).then_some(value)
}

#[cfg(test)]
mod tests;
