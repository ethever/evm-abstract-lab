//! 已知且可预期的长度精度缺口：<https://github.com/ethever/evm-abstract-lab/issues/58>。
//!
//! 缺口测试记录当前保守结果，并说明现有区间足以证明的更精确结果。
//! 修复该 issue 时应同步收紧断言；当前候选不表示每个值都有具体执行见证。

use super::ByteArray;
use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

fn byte_sequence(length: AbstractValue) -> ByteArray {
    // 表示层夹具，允许保留当前长度之外的稀疏事实；不是 EVM memory 写入轨迹。
    ByteArray {
        length,
        bytes: BTreeMap::from([(7, AbstractValue::constant(U256::from(0xaa)))]),
        default: AbstractValue::constant(U256::ZERO),
        memory: false,
    }
}

#[test]
fn known_precision_gap_upper_length_bound_keeps_out_of_range_byte() {
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
    // 当前 byte_at 只检查完整常量候选，因此仍保留可以排除的 0xaa。
    let actual = array.byte_at(7, domain);
    assert_eq!(
        actual.constants(),
        Some(&BTreeSet::from([U256::ZERO, U256::from(0xaa)]))
    );
}

#[test]
fn known_precision_gap_joined_length_lower_bound_adds_zero() {
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
    // 当前读取额外加入零；这是已知精度缺口，不是某条输入数组的真实字节。
    let actual = array.byte_at(7, domain);
    assert_eq!(
        actual.constants(),
        Some(&BTreeSet::from([U256::ZERO, U256::from(0xaa)]))
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
