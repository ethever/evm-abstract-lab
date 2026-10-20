//! Conservative byte observations and certified default numeric word packing.
//!
//! Each component transfer covers its input component, so their intersection
//! covers every concrete observed byte without exchanging facts. This private
//! observation boundary omits optional reduction; ordinary execution still uses
//! Domain::apply, including finite/exact byte observations. Word packing instead
//! proves equivalence to the default scalar fold under an explicit policy bound.

use super::{
    Domain, FiniteConstantSet, NumericValue, Profile, congruence::Congruence, interval::Interval,
    known_bits::KnownBits,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[cfg(test)]
use super::AbstractValue;

#[cfg(test)]
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
#[cfg(test)]
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

/// Cached masks/cardinalities only: no singleton, membership or certificate work.
/// The fixed fee covers 32 four-limb certificates, concatenation and the bounded
/// interval allocations. Each input member gets one mask test. Output subsets
/// need fixed-word arithmetic and BTreeSet insertion/comparison/allocation; a
/// binary-tree height bound overpays the actual B-tree. Overflow refuses.
pub(super) fn pack_work(domain: Domain, bytes: &[&NumericValue; 32]) -> usize {
    let mut fee = 1024usize + 32 * 256;
    let mut free = 0;
    for byte in bytes {
        fee = fee.saturating_add(byte.finite.work_size().saturating_mul(32));
        free += (!(byte.bits.zero() | byte.bits.one()) & U256::from(255)).count_ones();
        if domain.spec().profile() == Profile::ConstantsOnly {
            fee = fee.saturating_add(crate::domain::WordElement::singleton_work(*byte));
        }
    }
    if let Some(count) = cube_count(free, domain.capacity()) {
        let height = usize::BITS as usize - count.leading_zeros() as usize;
        fee = fee.saturating_add(count.saturating_mul(512 + 64 * height));
    }
    fee
}

fn cube_count(free: usize, capacity: usize) -> Option<usize> {
    u32::try_from(free)
        .ok()
        .and_then(|free| 1usize.checked_shl(free))
        .filter(|count| *count <= capacity)
}

/// Certify equality with the bit cube, rather than an upper bound on its image.
/// All 248 high bits are zero. Ranges admit both extrema; a dyadic congruence
/// fixes only already-fixed low bits. A finite set must be exactly the cube:
/// uniqueness + cardinality + one mask pass proves completeness without probing
/// all 256 byte values. Requiring equality also preserves the ordinary finite
/// transfer, which uses its raw finite operands before optional reductions.
fn byte_cube(value: &NumericValue) -> Option<KnownBits> {
    let bits = value.bits;
    let (one, maximum) = bits.unsigned_bounds();
    if maximum > U256::from(255)
        || !value.interval.contains(one)
        || !value.interval.contains(maximum)
        || (value.nonzero && one == U256::ZERO)
    {
        return None;
    }
    let free = !(bits.zero() | one);
    if let Some(exact) = value.congruence.singleton() {
        if free != U256::ZERO || exact != one {
            return None;
        }
    } else {
        let (modulus, residue) = value.congruence.modulus_residue()?;
        let low = modulus - U256::ONE;
        if !modulus.is_power_of_two() || free & low != U256::ZERO || one & low != residue {
            return None;
        }
    }
    if let Some(values) = value.constants()
        && (values.len() != 1usize << free.count_ones()
            || !values.iter().all(|v| bits.contains(*v)))
    {
        return None;
    }
    Some(bits)
}

/// Paid default-policy replacement for the ordered 32-byte SHL/OR fold. Metadata
/// remains the caller's responsibility. Disjoint lanes make each prefix exactly
/// a cube. Default mask/range/scalar cooperation yields its extrema, fixed low
/// prefix, nonzero flag and complete finite cover iff it fits capacity. Four
/// rounds and capacity+8 fact atoms cover those publications and confirmation;
/// smaller cooperation policies conservatively retain the ordinary fold.
pub(super) fn pack_numeric(domain: Domain, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
    if domain.spec().profile() == Profile::ConstantsOnly {
        let mut packed = [0_u8; 32];
        for (slot, byte) in packed.iter_mut().zip(bytes) {
            byte.finite.singleton()?;
            *slot = u8::try_from(byte.singleton()?).ok()?;
        }
        return Some(NumericValue::constant(U256::from_be_bytes(packed)));
    }
    let (mut zero, mut one) = (U256::ZERO, U256::ZERO);
    for (i, byte) in bytes.iter().enumerate() {
        let bits = byte_cube(byte)?;
        let shift = (31 - i) * 8;
        zero |= (bits.zero() & U256::from(255)) << shift;
        one |= bits.one() << shift;
    }
    let bits = KnownBits::new(zero, one).expect("disjoint certified byte lanes");
    if let Some(exact) = bits.singleton() {
        return Some(NumericValue::constant(exact));
    }
    let facts = domain.capacity().checked_add(8)?;
    domain
        .spec()
        .reduction_rounds()
        .checked_mul(3)?
        .checked_add(1)?;
    if domain.spec().reduction_rounds() < 4 || domain.spec().fact_limit() < facts {
        return None;
    }
    let sign = U256::ONE << 255usize;
    let biased_one = (one & !sign) | (zero & sign);
    let biased_zero = (zero & !sign) | (one & sign);
    let interval = Interval::new_unsigned(one, !zero)?
        .meet(&Interval::new_signed(biased_one, !biased_zero)?)?;
    let fixed_low = (zero | one).trailing_ones();
    let congruence = if fixed_low == 0 {
        Congruence::top()
    } else {
        let modulus = U256::ONE << fixed_low;
        Congruence::new(modulus, one & (modulus - U256::ONE))?
    };
    let free = !(zero | one);
    let finite = if cube_count(free.count_ones(), domain.capacity()).is_some() {
        let mut values = std::collections::BTreeSet::new();
        let mut subset = free;
        loop {
            values.insert(one | subset);
            if subset == U256::ZERO {
                break;
            }
            subset = (subset - U256::ONE) & free;
        }
        FiniteConstantSet::try_from_values(values).expect("cube contains its endpoints")
    } else {
        FiniteConstantSet::top()
    };
    Some(NumericValue {
        finite,
        bits,
        interval,
        congruence,
        nonzero: one != U256::ZERO,
    })
}

#[cfg(test)]
use super::provenance::Provenance;
#[cfg(test)]
mod tests;
