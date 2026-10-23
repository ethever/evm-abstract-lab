//! Exact replacements for the numeric word reader's 32 SHLs and 31 ORs.

use super::payment::{ReadPayment, ReadResult};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{EvmValue, provenance::Origin};
use crate::world::bytes::conditional::Context;

/// Reconstruct only cases equivalent to the scalar fold, including metadata.
/// The caller still owns the original word expression and its byte-state root.
#[cfg(test)]
pub(super) fn pack<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    bytes: &[EvmValue<N>],
    domain: AlgebraRef<'_, D>,
    context: &Context<'_, N, D>,
) -> Option<EvmValue<N>> {
    pack_paid(bytes, domain, context, &mut ReadPayment::unpaid())
        .ok()
        .flatten()
}

pub(super) fn pack_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    bytes: &[EvmValue<N>],
    domain: AlgebraRef<'_, D>,
    context: &Context<'_, N, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<Option<EvmValue<N>>> {
    let Ok(bytes): Result<&[EvmValue<N>; 32], _> = bytes.try_into() else {
        return Ok(None);
    };
    payer.context(context)?;
    let refs = std::array::from_fn(|i| bytes[i].numeric());
    let numeric = if payer.is_paid() {
        let Some(work) = domain.pack_word_work(&refs) else {
            return Ok(None);
        };
        payer.reserve(work)?;
        let out = domain.pack_word_numeric(&refs);
        payer.context(context)?;
        out
    } else {
        context.pack_word_numeric(domain, &refs)
    };
    let Some(numeric) = numeric else {
        payer.context(context)?;
        return Ok(None);
    };
    let mut value = EvmValue::from_numeric(numeric);
    let singleton = if payer.is_paid() {
        payer.singleton(&value)?
    } else {
        context.prove_singleton(&value)
    };
    payer.context(context)?;
    if domain.spec().relations().enabled && singleton.is_none() {
        return Ok(None);
    }
    let origins = bytes.iter().fold(16usize, |work, byte| {
        work.saturating_add(byte.provenance().origins().work_size().saturating_mul(64))
    });
    payer.reserve(origins)?;
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
    if singleton.is_none() && bytes.iter().any(EvmValue::symbolic_limit_reached) {
        value = value.with_symbolic_limit();
    }
    payer.context(context)?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests;
