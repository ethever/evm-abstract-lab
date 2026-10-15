//! Bounded literal arithmetic only; symbolic subgraphs keep backend validation.
//!
//! Constructors check widths/ranges before calling these helpers. Each literal
//! input and result is at most 512 bits, including before shift/mask allocation.
//! The only symbolic identity is a zero shift, which retains its entire input.

use super::{Bool, Bv, Op, Sort, Term};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};

const MAX_WIDTH: u32 = 512;

fn bounded(width: u32) -> bool {
    (1..=MAX_WIDTH).contains(&width)
}

fn literal(value: &Bv) -> Option<BigUint> {
    if !bounded(value.width()) || !value.0.0.args.is_empty() {
        return None;
    }
    let hex = value.literal_hex()?;
    if hex.len() > (MAX_WIDTH / 4) as usize {
        return None;
    }
    let parsed = BigUint::parse_bytes(hex.as_bytes(), 16)?;
    (parsed.bits() <= u64::from(value.width())).then_some(parsed)
}

fn result(value: BigUint, width: u32) -> Bv {
    Bv(Term::new(
        Op::Literal(value.to_str_radix(16)),
        Sort::Bv(width),
        vec![],
    ))
}

pub(super) fn binary(a: &Bv, b: &Bv, op: &Op) -> Option<Bv> {
    if !matches!(
        op,
        Op::Add | Op::Sub | Op::Mul | Op::Shl | Op::Lshr | Op::Ashr
    ) || !bounded(a.width())
        || a.width() != b.width()
    {
        return None;
    }
    let b = literal(b)?;
    if matches!(op, Op::Shl | Op::Lshr | Op::Ashr) && b.is_zero() {
        // Even a backend-invalid symbol/UF remains reachable and is reported.
        return Some(a.clone());
    }
    let a_value = literal(a)?;
    let width = a.width();
    let modulus = BigUint::one() << width;
    let mask = &modulus - BigUint::one();
    let value = match op {
        Op::Add => (a_value + b) % &modulus,
        Op::Sub => (a_value + &modulus - b) % &modulus,
        Op::Mul => (a_value * b) % &modulus,
        Op::Shl | Op::Lshr | Op::Ashr => {
            let negative = matches!(op, Op::Ashr) && a_value.bit(u64::from(width - 1));
            if b >= BigUint::from(width) {
                if negative { mask } else { BigUint::zero() }
            } else {
                let shift = b.to_u32().expect("shift below bounded width");
                match op {
                    Op::Shl => (a_value << shift) & mask,
                    Op::Lshr => a_value >> shift,
                    Op::Ashr if negative => (a_value >> shift) | (&mask ^ (&mask >> shift)),
                    Op::Ashr => a_value >> shift,
                    _ => unreachable!("shift operation"),
                }
            }
        }
        _ => unreachable!("supported bounded operation"),
    };
    Some(result(value, width))
}

pub(super) fn compare(a: &Bv, b: &Bv, op: &Op) -> Option<Bool> {
    if !matches!(op, Op::Eq | Op::Ult | Op::Ule | Op::Ugt | Op::Uge) || a.width() != b.width() {
        return None;
    }
    let a = literal(a)?;
    let b = literal(b)?;
    Some(Bool::from_bool(match op {
        Op::Eq => a == b,
        Op::Ult => a < b,
        Op::Ule => a <= b,
        Op::Ugt => a > b,
        Op::Uge => a >= b,
        _ => unreachable!("unsigned comparison"),
    }))
}

pub(super) fn zero_extend(value: &Bv, width: u32) -> Option<Bv> {
    if !bounded(width) {
        return None;
    }
    Some(result(literal(value)?, width))
}

pub(super) fn extract(value: &Bv, high: u32, low: u32) -> Option<Bv> {
    let value = literal(value)?;
    let width = high - low + 1;
    let mask = (BigUint::one() << width) - BigUint::one();
    Some(result((value >> low) & mask, width))
}

pub(super) fn boolean(values: &[Bool], conjunction: bool) -> Option<bool> {
    let mut result = conjunction;
    let mut all_literal = true;
    // Inspect every direct operand, even after the truth value is known.
    for value in values {
        if let Some(value) = value.as_bool() {
            if conjunction {
                result &= value;
            } else {
                result |= value;
            }
        } else {
            all_literal = false;
        }
    }
    all_literal.then_some(result)
}

pub(super) fn ite(guard: &Bool, yes: &Bv, no: &Bv) -> Option<Bv> {
    let condition = guard.as_bool()?;
    literal(yes)?;
    literal(no)?;
    Some(if condition { yes.clone() } else { no.clone() })
}

#[cfg(test)]
mod tests;
