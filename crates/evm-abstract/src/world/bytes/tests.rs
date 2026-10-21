//! Length and observed-cell numeric facts constrain the same immutable source.

use crate::world::test_support::TestBytes;
use crate::world::test_support::TestValue;
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

fn byte_sequence(length: TestValue) -> TestBytes {
    let mut array = TestBytes::unknown();
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
            numeric: TestValue::constant(U256::from(0xaa))
                .numeric()
                .clone()
                .into(),
        }],
        crate::domain::symbolic::StateLimits::default(),
    )
    .unwrap();
    array.bytes = BTreeMap::from([(7, TestValue::constant(U256::from(0xaa)))]);
    array.default = TestValue::unknown_byte();
    array
}

#[test]
fn upper_length_bound_proves_out_of_range_byte_is_zero() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let array = byte_sequence(TestValue::unsigned_range(U256::ZERO, U256::from(6)).unwrap());
    assert!(array.len().constants().is_none());
    assert!(!array.len().numeric().is_top());
    assert_eq!(
        array.len().interval().unsigned_bounds(),
        (U256::ZERO, U256::from(6))
    );

    // https://github.com/ethever/evm-abstract-lab/issues/58
    // 7 >= length 上界，精确结果应为 {0}。
    // The base binding and length upper bound exclude the observed byte.
    let actual = array.byte_at(7, domain.borrowed());
    assert_eq!(actual.constants(), Some(&BTreeSet::from([U256::ZERO])));
}

#[test]
fn joined_length_lower_bound_preserves_in_range_byte() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    // 九个具体数组在偏移 7 都是 0xaa；只通过公开构造和 join 形成摘要。
    // 第九个长度超出默认容量 8，有限组件变为 Top，区间仍为 [8,16]。
    let array = (9..=16).fold(TestBytes::exact(&[0xaa; 8]), |array, length| {
        array.join(&TestBytes::exact(&vec![0xaa; length]), domain.borrowed())
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
    let actual = array.byte_at(7, domain.borrowed());
    assert_eq!(
        actual.constants(),
        Some(&BTreeSet::from([U256::from(0xaa)]))
    );
}

#[test]
fn finite_lengths_distinguish_out_of_range_and_in_range_reads() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let out_of_range = domain.join(
        &TestValue::constant(U256::ZERO),
        &TestValue::constant(U256::from(6)),
    );
    let in_range = domain.join(
        &TestValue::constant(U256::from(8)),
        &TestValue::constant(U256::from(16)),
    );
    assert_eq!(
        byte_sequence(out_of_range)
            .byte_at(7, domain.borrowed())
            .singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        byte_sequence(in_range)
            .byte_at(7, domain.borrowed())
            .singleton(),
        Some(U256::from(0xaa))
    );
}

#[test]
fn uncertain_length_needs_zero_and_stored_byte() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    // 全未知长度与跨越边界的区间都确实允许越界和有效读取。
    for length in [
        TestValue::top(),
        TestValue::unsigned_range(U256::from(6), U256::from(8)).unwrap(),
    ] {
        let actual = byte_sequence(length).byte_at(7, domain.borrowed());
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
