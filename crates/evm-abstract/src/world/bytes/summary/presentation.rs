//! Refresh only the presentation of an already checked, freshly built root.

use super::{EvmByteArray, EvmValue, ExprError, ExprId, RangeError, restore_presentation, scalar};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
#[cfg(test)]
use crate::world::test_support::TestBytes;
use crate::{
    domain::symbolic::{ByteStateId, ByteStateKind, ExprKind},
    world::bytes::{numeric, zero},
};
use alloy_primitives::U256;
use std::collections::BTreeMap;
#[cfg(test)]
use {crate::world::test_support::TestAlgebra, crate::world::test_support::TestValue};

mod payment;
#[cfg(test)]
mod tests;
pub(super) use payment::refresh_with_payment;

pub(super) fn refresh<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: &mut EvmByteArray<N>,
    bare_cells: bool,
    domain: AlgebraRef<'_, D>,
) -> Result<(), RangeError> {
    refresh_with(out, bare_cells, domain, numeric::read)
}

fn refresh_with<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: &mut EvmByteArray<N>,
    bare_cells: bool,
    domain: AlgebraRef<'_, D>,
    mut read: impl FnMut(
        &ByteStateId,
        &ExprId,
        &BTreeMap<ExprId, EvmValue<N>>,
        AlgebraRef<'_, D>,
    ) -> EvmValue<N>,
) -> Result<(), RangeError> {
    // Constructor knowledge matters even when chunk/slice canonicalization
    // happens to leave an Input root. No other root or cache is inspected.
    let input = if bare_cells
        && !out.bytes.is_empty()
        && let ByteStateKind::Input {
            length, bindings, ..
        } = out.root.kind()
        && bindings.len() == out.bytes.len()
    {
        // Use exactly the Reader's length lookup after the fresh operand was
        // installed, with relations disabled; do not project old summary facts.
        let length = numeric::scalar(length, &out.operands, numeric::numeric_domain(domain));
        Some((bindings, crate::world::unsigned_bounds(&length)))
    } else {
        None
    };
    // Keep the original key snapshot, cancellation boundary and copy lifecycle.
    // Actual reads still attach the executable state term only on demand.
    for index in out.bytes.keys().copied().collect::<Vec<_>>() {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        let at = U256::from(index);
        let value = input
            .and_then(|(bindings, (lower, upper))| {
                // Ambiguous bounds already require Reader; avoid searching
                // bindings or validating a carrier on that fallback path.
                if lower <= at && at < upper {
                    return None;
                }
                let binding = &bindings[bindings.binary_search_by_key(&at, |b| b.index).ok()?];
                if !matches!(binding.value.kind(), ExprKind::Fresh(_))
                    || binding.numeric.interval().unsigned_bounds().1 > U256::from(255)
                {
                    return None;
                }
                if at >= upper {
                    Some(zero())
                } else {
                    // Q is the actual emitted carrier, including every numeric
                    // component. Final scalar below discards all read metadata.
                    // Preserve the actual carrier for this exact binding Expr.
                    // The element owns default/raw versus custom import policy.
                    let numeric = out
                        .operands
                        .get(&binding.value)
                        .map(EvmValue::numeric)
                        .map(|value| value.import_binding(&binding.numeric))
                        .unwrap_or_else(|| N::from_observation(&binding.numeric));
                    Some(EvmValue::<N>::from_numeric(numeric))
                }
            })
            .unwrap_or_else(|| read(&out.root, &ExprId::constant(at), &out.operands, domain));
        let value = restore_presentation(value, out.bytes.get(&index));
        let value = scalar(&value, domain);
        out.bytes.insert(index, value);
    }
    Ok(())
}
