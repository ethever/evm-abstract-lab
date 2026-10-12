//! Word predicates for mathematical byte ranges, before memory effects execute.
//!
//! Subtraction is guarded by the offset bound. This expresses a checked sum
//! without allowing EVM's modular arithmetic to wrap an invalid range to zero.

use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;
use revm_bytecode::opcode;

/// A nonempty access must fit the word-rounded logical memory policy. An empty
/// access never expands memory, even when its offset is not host representable.
pub(super) fn memory_condition(
    offset: &AbstractValue,
    size: &AbstractValue,
    maximum: usize,
    domain: Domain,
) -> AbstractValue {
    let limit = AbstractValue::constant(U256::from(maximum - maximum % 32));
    let empty = domain.apply(opcode::ISZERO, std::slice::from_ref(size));
    let fits = sequence_condition(offset, size, &limit, domain);
    domain.apply(opcode::OR, &[empty, fits])
}

/// A returndata read must fit even for size zero: an offset beyond the end is
/// exceptional. The predicate also rejects an overflowing offset + size.
pub(super) fn sequence_condition(
    offset: &AbstractValue,
    size: &AbstractValue,
    length: &AbstractValue,
    domain: Domain,
) -> AbstractValue {
    let offset_fits = at_most(offset, length, domain);
    let remaining = domain.apply(opcode::SUB, &[length.clone(), offset.clone()]);
    let size_fits = at_most(size, &remaining, domain);
    domain.apply(opcode::AND, &[offset_fits, size_fits])
}

fn at_most(left: &AbstractValue, right: &AbstractValue, domain: Domain) -> AbstractValue {
    let greater = domain.apply(opcode::GT, &[left.clone(), right.clone()]);
    domain.apply(opcode::ISZERO, &[greater])
}

#[cfg(test)]
mod tests;
