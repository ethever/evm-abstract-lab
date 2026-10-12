use super::{memory_condition, sequence_condition};
use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;

fn value(value: U256) -> AbstractValue {
    AbstractValue::constant(value)
}

#[test]
fn sequence_guard_agrees_with_checked_math_including_empty_and_overflow() {
    let cases = [
        U256::ZERO,
        U256::from(1),
        U256::from(31),
        U256::from(32),
        U256::MAX - U256::from(1),
        U256::MAX,
    ];
    for offset in cases {
        for size in cases {
            for length in cases {
                let expected = offset.checked_add(size).is_some_and(|end| end <= length);
                let actual = sequence_condition(
                    &value(offset),
                    &value(size),
                    &value(length),
                    Domain::default(),
                );
                assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
            }
        }
    }
}

#[test]
fn memory_guard_distinguishes_zero_size_and_word_rounded_capacity() {
    let domain = Domain::default();
    for (offset, size, cap, expected) in [
        (U256::MAX, 0, 0, true),
        (U256::ZERO, 1, 31, false),
        (U256::ZERO, 1, 32, true),
        (U256::from(31), 1, 32, true),
        (U256::from(31), 2, 63, false),
        (U256::from(31), 2, 64, true),
        (U256::MAX, 1, usize::MAX, false),
    ] {
        let actual = memory_condition(&value(offset), &value(U256::from(size)), cap, domain);
        assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
    }
}
