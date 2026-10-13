//! Length and observed-cell numeric facts constrain the same immutable source.

use super::ByteArray;
use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

fn byte_sequence(length: AbstractValue) -> ByteArray {
    let mut array = ByteArray::unknown();
    let length_expr = length
        .symbolic_expression()
        .unwrap_or_else(|| crate::domain::symbolic::ExprId::fresh().unwrap());
    array.length = length.clone().with_expression(length_expr.clone());
    array.operands.insert(length_expr.clone(), length);
    array.root = crate::domain::symbolic::ByteStateId::observed(
        crate::domain::symbolic::SourceId::fresh().unwrap(),
        length_expr,
        vec![crate::domain::symbolic::StateBinding {
            index: U256::from(7),
            value: crate::domain::symbolic::ExprId::constant(U256::from(0xaa)),
            numeric: AbstractValue::constant(U256::from(0xaa)).numeric().clone(),
        }],
        crate::domain::symbolic::StateLimits::default(),
    )
    .unwrap();
    array.bytes = BTreeMap::from([(7, AbstractValue::constant(U256::from(0xaa)))]);
    array.default = AbstractValue::unknown_byte();
    array
}

#[test]
fn upper_length_bound_proves_out_of_range_byte_is_zero() {
    let domain = Domain::default();
    let array = byte_sequence(AbstractValue::unsigned_range(U256::ZERO, U256::from(6)).unwrap());
    assert!(array.len().constants().is_none());
    assert!(!array.len().numeric().is_top());
    assert_eq!(
        array.len().interval().unsigned_bounds(),
        (U256::ZERO, U256::from(6))
    );

    // https://github.com/ethever/evm-abstract-lab/issues/58
    // 7 >= length 上界，精确结果应为 {0}。
    // The base binding and length upper bound exclude the observed byte.
    let actual = array.byte_at(7, domain);
    assert_eq!(actual.constants(), Some(&BTreeSet::from([U256::ZERO])));
}

#[test]
fn joined_length_lower_bound_preserves_in_range_byte() {
    let domain = Domain::default();
    // 九个具体数组在偏移 7 都是 0xaa；只通过公开构造和 join 形成摘要。
    // 第九个长度超出默认容量 8，有限组件变为 Top，区间仍为 [8,16]。
    let array = (9..=16).fold(ByteArray::exact(&[0xaa; 8]), |array, length| {
        array.join(&ByteArray::exact(&vec![0xaa; length]), domain)
    });
    assert!(array.len().constants().is_none());
    assert!(!array.len().numeric().is_top());
    assert_eq!(
        array.len().interval().unsigned_bounds(),
        (U256::from(8), U256::from(16))
    );
    assert_eq!(
        array.bytes.get(&7).unwrap().singleton(),
        Some(U256::from(0xaa))
    );

    // https://github.com/ethever/evm-abstract-lab/issues/58
    // 7 < length 下界，精确结果应为 {0xaa}。
    // Every joined source contains this byte; no padding alternative applies.
    let actual = array.byte_at(7, domain);
    assert_eq!(
        actual.constants(),
        Some(&BTreeSet::from([U256::from(0xaa)]))
    );
}

#[test]
fn finite_lengths_distinguish_out_of_range_and_in_range_reads() {
    let domain = Domain::default();
    let out_of_range = domain.join(
        &AbstractValue::constant(U256::ZERO),
        &AbstractValue::constant(U256::from(6)),
    );
    let in_range = domain.join(
        &AbstractValue::constant(U256::from(8)),
        &AbstractValue::constant(U256::from(16)),
    );
    assert_eq!(
        byte_sequence(out_of_range).byte_at(7, domain).singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        byte_sequence(in_range).byte_at(7, domain).singleton(),
        Some(U256::from(0xaa))
    );
}

#[test]
fn uncertain_length_needs_zero_and_stored_byte() {
    let domain = Domain::default();
    // 全未知长度与跨越边界的区间都确实允许越界和有效读取。
    for length in [
        AbstractValue::top(),
        AbstractValue::unsigned_range(U256::from(6), U256::from(8)).unwrap(),
    ] {
        let actual = byte_sequence(length).byte_at(7, domain);
        assert_eq!(
            actual.constants(),
            Some(&BTreeSet::from([U256::ZERO, U256::from(0xaa)]))
        );
    }
}

mod concrete;
mod copies;
mod fingerprint;
mod guarded;
mod native;
mod provenance;
mod refinement;
mod scoped;
mod state;
mod summary;
mod widening;
