//! Exact replacements for the numeric word reader's 32 SHLs and 31 ORs.

use crate::domain::{AbstractValue, Domain, NumericValue, Profile, provenance::Origin};
use alloy_primitives::U256;

/// Reconstruct only cases equivalent to the scalar fold, including metadata.
/// The caller still owns the original word expression and its byte-state root.
pub(super) fn pack(bytes: &[AbstractValue], domain: Domain) -> Option<AbstractValue> {
    let bytes: &[AbstractValue; 32] = bytes.try_into().ok()?;
    let numeric = if let Some(word) = concrete(bytes, domain) {
        NumericValue::constant(word)
    } else {
        // An exact shape check retains finite, interval, congruence and nonzero
        // restrictions. Enabled relations can reconstruct correlated expressions.
        let unknown = NumericValue::unknown_byte();
        if domain.spec().relations().enabled || bytes.iter().any(|b| b.numeric() != &unknown) {
            return None;
        }
        // All 256 bits vary independently in this numeric fold. Its result is
        // Top in both profiles, independently of reduction/fact capacities.
        NumericValue::top()
    };

    // Every SHL adds Constant (its shift) and Arithmetic. OR unions all byte
    // sources. Construct a fresh value: neither input/copy identity, expression
    // nor the code-address role survives these arithmetic operations.
    let mut value = AbstractValue::constant(U256::ZERO)
        .with_numeric(numeric)
        .with_origin(Origin::Arithmetic);
    for byte in bytes {
        let Some(origins) = byte.provenance().origins().sources() else {
            value = AbstractValue::from_numeric(value.numeric().clone());
            break;
        };
        for origin in origins {
            value = value.with_origin(*origin);
        }
    }
    // apply propagates this marker even with relations disabled, but every
    // concrete SHL clears it. Do not use project, which would clear it on Top.
    if value.singleton().is_none() && bytes.iter().any(AbstractValue::symbolic_limit_reached) {
        value = value.with_symbolic_limit();
    }
    Some(value)
}

fn concrete(bytes: &[AbstractValue; 32], domain: Domain) -> Option<U256> {
    let mut packed = [0_u8; 32];
    for (slot, byte) in packed.iter_mut().zip(bytes) {
        // ConstantsOnly ignores the other numeric components in finite_apply.
        // A singleton proved only by those components cannot replace its fold.
        if domain.spec().profile() == Profile::ConstantsOnly
            && byte.constants().is_none_or(|values| values.len() != 1)
        {
            return None;
        }
        *slot = u8::try_from(byte.singleton()?).ok()?;
    }
    Some(U256::from_be_bytes(packed))
}

#[cfg(test)]
mod tests;
