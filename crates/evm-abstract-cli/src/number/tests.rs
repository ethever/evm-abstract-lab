//! Number boundary tests include values beyond u64 and lexical rejection cases.

use super::{NumberError, parse};
use alloy_primitives::U256;

const MAX_DECIMAL: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

#[test]
fn decimal_and_hex_preserve_the_full_unsigned_range() {
    let above_u64 = U256::from(1) << 64;
    for (input, expected) in [
        ("0", U256::ZERO),
        ("000", U256::ZERO),
        ("010", U256::from(10)),
        ("56", U256::from(56)),
        ("0x38", U256::from(56)),
        ("0X38", U256::from(56)),
        ("0xaB", U256::from(171)),
        ("0XAb", U256::from(171)),
        ("18446744073709551616", above_u64),
        ("0x10000000000000000", above_u64),
        (MAX_DECIMAL, U256::MAX),
    ] {
        assert_eq!(parse(input).unwrap(), expected, "input: {input}");
    }
    assert_eq!(parse(&format!("0x{}", "f".repeat(64))).unwrap(), U256::MAX);
    assert_eq!(parse(&format!("0X{}", "F".repeat(64))).unwrap(), U256::MAX);
}

#[test]
fn leading_zeros_are_limited_by_numeric_range_rather_than_literal_length() {
    let zeros = "0".repeat(100);
    assert_eq!(parse(&zeros).unwrap(), U256::ZERO);
    assert_eq!(parse(&format!("{zeros}{MAX_DECIMAL}")).unwrap(), U256::MAX);
    assert_eq!(
        parse(&format!("0x{zeros}{}", "f".repeat(64))).unwrap(),
        U256::MAX
    );
}

#[test]
fn rejects_unsigned_256_bit_overflow_in_both_bases() {
    let above_max =
        "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    for input in [above_max.to_owned(), format!("0x1{}", "0".repeat(64))] {
        assert!(
            matches!(parse(&input), Err(NumberError::Overflow { .. })),
            "input: {input}"
        );
    }
}

#[test]
fn rejects_signs_whitespace_other_radices_and_non_ascii_digits() {
    for input in [
        "", "0x", "0X", "-0", "-1", "+0", "+1", " 1", "1 ", "1\n", "0x 1", "0x1 ", "1.0", "1e3",
        "1E3", "1_000", "0x1_0", "0b10", "0o10", "ff", "0xgg", "０", "١", "0x１", "1:2",
    ] {
        assert!(
            matches!(parse(input), Err(NumberError::Syntax)),
            "input: {input:?}"
        );
    }
}
