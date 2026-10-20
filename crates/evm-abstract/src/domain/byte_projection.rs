//! Conservative byte observations for the content reader's numeric policy.
//!
//! Each component transfer covers its input component, so their intersection
//! covers every concrete observed byte without exchanging facts. This private
//! boundary deliberately omits optional reduction; ordinary execution still
//! uses Domain::apply, including all finite and exact scalar inputs here.

use super::{
    AbstractValue, Domain, FiniteConstantSet, NumericValue, Profile, congruence::Congruence,
    interval::Interval, known_bits::KnownBits,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

impl Domain {
    /// Compatibility entrypoint; numeric primitives never re-enter this facade.
    pub(crate) fn observe_byte(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        if projection_scalar(*self, op, args).is_none() {
            return self.algebra().apply(op, args);
        }
        self.algebra().observe_byte(op, args)
    }
}

pub(super) fn observe(domain: Domain, op: u8, args: &[NumericValue]) -> NumericValue {
    observe_with(domain, op, args, || domain.numeric_apply(op, args).value)
}

pub(super) fn observe_with(
    domain: Domain,
    op: u8,
    args: &[NumericValue],
    fallback: impl FnOnce() -> NumericValue,
) -> NumericValue {
    let [left, right] = args else {
        return fallback();
    };
    if projection_side(domain, op, left, right).is_none() {
        return fallback();
    }
    let value = NumericValue {
        finite: FiniteConstantSet::top(),
        bits: KnownBits::transfer(op, &[args[0].bits, args[1].bits]),
        interval: Interval::transfer(op, &[args[0].interval, args[1].interval]),
        congruence: Congruence::transfer(
            op,
            &[args[0].congruence.clone(), args[1].congruence.clone()],
        ),
        nonzero: false,
    };
    value.singleton().map_or(value, NumericValue::constant)
}

fn projection_side(
    domain: Domain,
    op: u8,
    left: &NumericValue,
    right: &NumericValue,
) -> Option<bool> {
    if domain.spec().profile() != Profile::Product {
        return None;
    }
    let literal = |value: &NumericValue| {
        value
            .finite_constants()
            .singleton()
            .filter(|v| value.contains(*v))
    };
    let (scalar, is_left) = match op {
        opcode::BYTE if literal(left).is_some() => (right, false),
        opcode::AND if literal(right) == Some(U256::from(255)) => (left, true),
        _ => return None,
    };
    (scalar.constants().is_none() && scalar.singleton().is_none()).then_some(is_left)
}

// The legacy facade still gates this shortcut on its effective expression policy.
fn projection_scalar(domain: Domain, op: u8, args: &[AbstractValue]) -> Option<&AbstractValue> {
    if domain.spec().relations().enabled {
        return None;
    }
    let [left, right] = args else { return None };
    projection_side(domain, op, left.numeric(), right.numeric())
        .map(|is_left| if is_left { left } else { right })
}

/// Fixed-width numeric-support proof for a union containing every byte.
/// No numeric candidate enumeration or execution reachability is asserted.
pub(super) fn full_byte_image(value: &NumericValue) -> bool {
    let zero = value.known_bits().zero();
    let one = value.known_bits().one();
    if one & zero != U256::ZERO {
        return false;
    }
    let (lower, upper) = value.interval().unsigned_bounds();
    if lower > one || !zero > upper {
        return false;
    }
    let sign = U256::ONE << 255usize;
    let biased_one = (one & !sign) | (zero & sign);
    let biased_zero = (zero & !sign) | (one & sign);
    let (lower, upper) = value.interval().signed_bounds();
    lower <= biased_one
        && !biased_zero <= upper
        && (!(one | zero)).to_le_bytes::<32>().contains(&u8::MAX)
}

/// Numeric fold replacement. Metadata remains the caller's responsibility.
pub(super) fn pack_numeric(domain: Domain, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
    let mut packed = [0_u8; 32];
    let concrete = packed.iter_mut().zip(bytes).all(|(slot, byte)| {
        if domain.spec().profile() == Profile::ConstantsOnly
            && byte.constants().is_none_or(|values| values.len() != 1)
        {
            return false;
        }
        let Some(byte) = byte.singleton().and_then(|v| u8::try_from(v).ok()) else {
            return false;
        };
        *slot = byte;
        true
    });
    if concrete {
        return Some(NumericValue::constant(U256::from_be_bytes(packed)));
    }
    bytes
        .iter()
        .all(|byte| *byte == &NumericValue::unknown_byte())
        .then(NumericValue::top)
}

#[cfg(test)]
use super::provenance::Provenance;
#[cfg(test)]
mod tests;
