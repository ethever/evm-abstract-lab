use super::KnownBits;
use alloy_primitives::U256;
use proptest::prelude::*;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

const BINARY_OPS: [u8; 24] = [
    opcode::ADD,
    opcode::MUL,
    opcode::SUB,
    opcode::DIV,
    opcode::SDIV,
    opcode::MOD,
    opcode::SMOD,
    opcode::EXP,
    opcode::SIGNEXTEND,
    opcode::LT,
    opcode::GT,
    opcode::SLT,
    opcode::SGT,
    opcode::EQ,
    opcode::AND,
    opcode::OR,
    opcode::XOR,
    opcode::BYTE,
    opcode::SHL,
    opcode::SHR,
    opcode::SAR,
    opcode::ADDMOD,
    opcode::MULMOD,
    0xff,
];

fn concrete(op: u8, a: U256, b: U256, c: U256) -> U256 {
    super::super::evaluate(op, a, b, c)
}

fn low_patterns() -> Vec<(KnownBits, Vec<U256>)> {
    (0_u32..81)
        .map(|mut pattern| {
            let mut zero = !U256::from(15);
            let mut one = U256::ZERO;
            for index in 0..4 {
                let mask = U256::from(1) << index;
                match pattern % 3 {
                    0 => zero |= mask,
                    1 => one |= mask,
                    _ => {}
                }
                pattern /= 3;
            }
            let bits = KnownBits::new(zero, one).unwrap();
            let values = (0_u8..16)
                .map(U256::from)
                .filter(|value| bits.contains(*value))
                .collect();
            (bits, values)
        })
        .collect()
}

// 从读者看到的语法独立枚举每个位置的候选数字，不读取 KnownBits 掩码。
fn displayed_nibbles(rendered: &str) -> Vec<BTreeSet<u8>> {
    let mut characters = rendered.strip_prefix("0x").unwrap().chars();
    let mut nibbles = Vec::new();
    while let Some(character) = characters.next() {
        let values = match character {
            '*' => (0..16).collect(),
            '[' => {
                let pattern: Vec<_> = characters.by_ref().take(4).collect();
                assert_eq!(pattern.len(), 4);
                assert!(pattern.iter().all(|bit| matches!(bit, '0' | '1' | '*')));
                assert!(pattern.contains(&'*'));
                assert!(pattern.iter().any(|bit| *bit != '*'));
                assert_eq!(characters.next(), Some(']'));
                (0_u8..16)
                    .filter(|value| {
                        format!("{value:04b}")
                            .chars()
                            .zip(&pattern)
                            .all(|(actual, known)| *known == '*' || *known == actual)
                    })
                    .collect()
            }
            digit => {
                assert!(matches!(digit, '0'..='9' | 'a'..='f'));
                BTreeSet::from([digit.to_digit(16).unwrap() as u8])
            }
        };
        nibbles.push(values);
    }
    assert_eq!(nibbles.len(), 64);
    nibbles
}

#[test]
fn display_preserves_all_nibble_constraints_at_every_word_position() {
    for (pattern, values) in low_patterns() {
        let expected = values
            .into_iter()
            .map(|value| value.to::<u8>())
            .collect::<BTreeSet<_>>();
        for index in 0..64 {
            let shift = index * 4;
            let zero = (pattern.zero() & U256::from(15)) << shift;
            let one = pattern.one() << shift;
            let bits = KnownBits::new(zero, one).unwrap();
            let rendered = bits.to_string();
            let decoded = displayed_nibbles(&rendered);
            for (display_index, actual) in decoded.iter().enumerate() {
                if display_index == 63 - index {
                    assert_eq!(actual, &expected, "{rendered}");
                    for candidate in 0_u8..16 {
                        assert_eq!(
                            actual.contains(&candidate),
                            bits.contains(U256::from(candidate) << shift),
                            "{rendered}, candidate={candidate:x}"
                        );
                    }
                } else {
                    assert_eq!(actual, &(0..16).collect::<BTreeSet<_>>(), "{rendered}");
                }
            }
        }
    }
}

#[test]
fn display_keeps_all_unknown_positions_and_exact_hex_digits() {
    assert_eq!(
        KnownBits::top().to_string(),
        format!("0x{}", "*".repeat(64))
    );
    assert_eq!(
        KnownBits::exact(U256::ZERO).to_string(),
        format!("0x{}", "0".repeat(64))
    );
    assert_eq!(
        KnownBits::exact(U256::MAX).to_string(),
        format!("0x{}", "f".repeat(64))
    );
    let word = U256::from_str_radix(
        "fedcba98765432100123456789abcdefdeadbeef23456789abcdef0123456789",
        16,
    )
    .unwrap();
    assert_eq!(KnownBits::exact(word).to_string(), format!("{word:#066x}"));
}

#[test]
fn display_orders_mixed_nibbles_and_partial_binary_bits_high_to_low() {
    let zero = (U256::from(5) << 252) | (U256::from(8) << 248) | U256::from(0x40);
    let one = (U256::from(10) << 252) | (U256::from(4) << 248) | U256::from(0x8f);
    let bits = KnownBits::new(zero, one).unwrap();
    assert_eq!(
        bits.to_string(),
        format!("0xa[01**]{}[10**]f", "*".repeat(60))
    );
    let clz = KnownBits::from_unsigned_bounds(U256::ZERO, U256::from(256));
    assert_eq!(clz.to_string(), format!("0x{}[000*]**", "0".repeat(61)));
}

#[test]
fn display_preserves_serialized_zero_and_one_masks() {
    let zero = U256::from(8);
    let one = U256::from(4);
    let bits = KnownBits::new(zero, one).unwrap();
    assert_eq!(bits.to_string(), format!("0x{}[01**]", "*".repeat(63)));
    assert_eq!(
        serde_json::to_value(bits).unwrap(),
        serde_json::json!({ "zero": zero, "one": one })
    );
}

#[test]
fn invariant_join_and_meet_are_distinct() {
    let bit: U256 = U256::from(1) << 255;
    assert!(KnownBits::new(bit, bit).is_none());
    let zero = KnownBits::exact(U256::ZERO);
    let high = KnownBits::exact(bit);
    assert!(zero.meet(&high).is_none());
    let both = zero.join(&high);
    assert_eq!(both.zero(), !bit);
    assert_eq!(both.one(), U256::ZERO);
    assert!(both.contains(U256::ZERO));
    assert!(both.contains(bit));
    assert!(!both.contains(U256::from(1)));
    assert_eq!(both.meet(&high), Some(high));
}

#[test]
fn finite_sets_and_bounds_have_sound_full_width_bits() {
    let set = BTreeSet::from([U256::from(0x12), U256::from(0x16)]);
    let bits = KnownBits::from_values(&set);
    assert_eq!(bits.zero(), !U256::from(0x16));
    assert_eq!(bits.one(), U256::from(0x12));
    assert_eq!(bits.unsigned_bounds(), (U256::from(0x12), U256::from(0x16)));
    assert_eq!(KnownBits::from_values(&BTreeSet::new()), KnownBits::top());
    let sign: U256 = U256::from(1) << 255;
    let positive = KnownBits::from_unsigned_bounds(U256::ZERO, sign - U256::from(1));
    let negative = KnownBits::from_unsigned_bounds(sign, U256::MAX);
    assert_eq!(positive.zero(), sign);
    assert_eq!(negative.one(), sign);
    assert_eq!(positive.join(&negative), KnownBits::top());
    assert_eq!(
        KnownBits::from_unsigned_bounds(sign, sign).singleton(),
        Some(sign)
    );
    assert_eq!(
        KnownBits::from_unsigned_bounds(U256::MAX, U256::ZERO),
        KnownBits::top()
    );
}

#[test]
fn all_four_low_bit_patterns_preserve_concrete_results() {
    let patterns = low_patterns();
    for (a, a_values) in &patterns {
        for op in [opcode::NOT, opcode::ISZERO, opcode::CLZ] {
            let result = KnownBits::transfer(op, &[*a]);
            for value in a_values {
                assert!(result.contains(concrete(op, *value, U256::ZERO, U256::ZERO)));
            }
        }
        for (b, b_values) in &patterns {
            // 实际 U256 运算包含 SUB 的 256 位环绕，不是 4 位 toy 语义。
            for op in BINARY_OPS[..21].iter().copied() {
                let result = KnownBits::transfer(op, &[*a, *b]);
                for a_value in a_values {
                    for b_value in b_values {
                        let value = concrete(op, *a_value, *b_value, U256::ZERO);
                        assert!(
                            result.contains(value),
                            "op={op:x}, a={a:?}, b={b:?}, concrete={value:x}, result={result:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn modular_transfer_respects_full_sum_and_product() {
    let a = KnownBits::exact(U256::MAX).join(&KnownBits::exact(U256::MAX - U256::from(1)));
    let b = KnownBits::exact(U256::from(2)).join(&KnownBits::exact(U256::from(3)));
    for modulus in [0_u64, 1, 2, 3, 8, 17, 257] {
        let modulus = U256::from(modulus);
        for op in [opcode::ADDMOD, opcode::MULMOD] {
            let result = KnownBits::transfer(op, &[a, b, KnownBits::exact(modulus)]);
            for left in [U256::MAX, U256::MAX - U256::from(1)] {
                for right in [U256::from(2), U256::from(3)] {
                    assert!(result.contains(concrete(op, left, right, modulus)));
                }
            }
        }
    }
    // EVM ADDMOD 不先把相加结果截成 256 位；奇数模数会暴露区别。
    let expected = concrete(opcode::ADDMOD, U256::MAX, U256::from(2), U256::from(3));
    assert_ne!(
        expected,
        U256::MAX.wrapping_add(U256::from(2)) % U256::from(3)
    );
}

#[test]
fn arithmetic_identities_survive_an_unknown_operand() {
    let unknown = KnownBits::top();
    let zero = KnownBits::exact(U256::ZERO);
    let one = KnownBits::exact(U256::from(1));
    let maximum = KnownBits::exact(U256::MAX);
    for op in [opcode::MUL, opcode::AND] {
        assert_eq!(KnownBits::transfer(op, &[unknown, zero]), zero);
        assert_eq!(KnownBits::transfer(op, &[zero, unknown]), zero);
    }
    assert_eq!(
        KnownBits::transfer(opcode::OR, &[unknown, maximum]),
        maximum
    );
    assert_eq!(KnownBits::transfer(opcode::DIV, &[unknown, zero]), zero);
    assert_eq!(KnownBits::transfer(opcode::MOD, &[unknown, one]), zero);
    assert_eq!(KnownBits::transfer(opcode::EXP, &[unknown, zero]), one);
    assert_eq!(KnownBits::transfer(opcode::EXP, &[one, unknown]), one);
    assert_eq!(KnownBits::transfer(opcode::DIV, &[unknown, one]), unknown);
    assert_eq!(KnownBits::transfer(opcode::SDIV, &[unknown, one]), unknown);
    assert_eq!(
        KnownBits::transfer(opcode::ADDMOD, &[zero, zero, unknown]),
        zero
    );
    assert_eq!(
        KnownBits::transfer(opcode::MULMOD, &[zero, unknown, unknown]),
        zero
    );
}

#[test]
fn carry_propagation_and_wrapping_keep_guaranteed_bits() {
    let even = KnownBits::new(U256::from(1), U256::ZERO).unwrap();
    let odd = KnownBits::new(U256::ZERO, U256::from(1)).unwrap();
    let one = KnownBits::exact(U256::from(1));
    assert!(KnownBits::transfer(opcode::ADD, &[even, one]).one().bit(0));
    assert!(KnownBits::transfer(opcode::ADD, &[odd, one]).zero().bit(0));
    assert!(KnownBits::transfer(opcode::SUB, &[even, one]).one().bit(0));
    assert!(KnownBits::transfer(opcode::SUB, &[odd, one]).zero().bit(0));
    let both = KnownBits::transfer(opcode::MUL, &[even, even]);
    assert_eq!(both.zero() & U256::from(3), U256::from(3));
    assert!(both.contains(U256::MAX - U256::from(3)));
    let exponent = KnownBits::new(U256::ZERO, U256::from(256)).unwrap();
    assert_eq!(
        KnownBits::transfer(opcode::EXP, &[even, exponent]).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn comparisons_do_not_infer_identity_from_equal_abstract_inputs() {
    let alternatives = KnownBits::exact(U256::from(1)).join(&KnownBits::exact(U256::from(3)));
    let equal = KnownBits::transfer(opcode::EQ, &[alternatives, alternatives]);
    let xor = KnownBits::transfer(opcode::XOR, &[alternatives, alternatives]);
    assert_eq!(equal.singleton(), None);
    assert!(equal.contains(U256::ZERO));
    assert!(equal.contains(U256::from(1)));
    assert!(xor.contains(U256::ZERO));
    assert!(xor.contains(U256::from(2)));
    let negative = KnownBits::new(U256::ZERO, U256::from(1) << 255).unwrap();
    let positive = KnownBits::new(U256::from(1) << 255, U256::ZERO).unwrap();
    assert_eq!(
        KnownBits::transfer(opcode::SLT, &[negative, positive]).singleton(),
        Some(U256::from(1))
    );
    assert_eq!(
        KnownBits::transfer(opcode::LT, &[negative, positive]).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn large_shifts_and_unknown_sign_use_evm_semantics() {
    let unknown = KnownBits::top();
    let negative = KnownBits::new(U256::ZERO, U256::from(1) << 255).unwrap();
    let positive = KnownBits::new(U256::from(1) << 255, U256::ZERO).unwrap();
    for shift in [U256::from(256), U256::from(257), U256::MAX] {
        let shift = KnownBits::exact(shift);
        for op in [opcode::SHL, opcode::SHR] {
            assert_eq!(
                KnownBits::transfer(op, &[shift, unknown]).singleton(),
                Some(U256::ZERO)
            );
        }
        assert_eq!(
            KnownBits::transfer(opcode::SAR, &[shift, negative]).singleton(),
            Some(U256::MAX)
        );
        assert_eq!(
            KnownBits::transfer(opcode::SAR, &[shift, positive]).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(KnownBits::transfer(opcode::SAR, &[shift, unknown]), unknown);
    }
    let shifts = KnownBits::exact(U256::from(255)).join(&KnownBits::exact(U256::from(511)));
    let result = KnownBits::transfer(opcode::SAR, &[shifts, negative]);
    assert_eq!(result.singleton(), Some(U256::MAX));
    // 未知但必非零的移位量仍能固定逻辑左移的低位。
    let odd_shift = KnownBits::new(U256::ZERO, U256::from(1)).unwrap();
    assert!(
        KnownBits::transfer(opcode::SHL, &[odd_shift, unknown])
            .zero()
            .bit(0)
    );
}

#[test]
fn byte_is_zero_extended_and_signextend_observes_its_sign_bit() {
    let unknown = KnownBits::top();
    let result = KnownBits::transfer(opcode::BYTE, &[unknown, unknown]);
    assert_eq!(result.zero(), !U256::from(255));
    assert!(result.contains(U256::from(255)));
    assert!(!result.contains(U256::from(256)));
    assert_eq!(
        KnownBits::transfer(opcode::BYTE, &[KnownBits::exact(U256::from(32)), unknown]).singleton(),
        Some(U256::ZERO)
    );
    let negative_byte = KnownBits::new(U256::ZERO, U256::from(128)).unwrap();
    let positive_byte = KnownBits::new(U256::from(128), U256::ZERO).unwrap();
    let zero = KnownBits::exact(U256::ZERO);
    assert_eq!(
        KnownBits::transfer(opcode::SIGNEXTEND, &[zero, negative_byte]).one(),
        !U256::from(127)
    );
    assert_eq!(
        KnownBits::transfer(opcode::SIGNEXTEND, &[zero, positive_byte]).zero(),
        !U256::from(127)
    );
    assert_eq!(
        KnownBits::transfer(
            opcode::SIGNEXTEND,
            &[KnownBits::exact(U256::from(32)), negative_byte]
        ),
        negative_byte
    );
}

#[test]
fn clz_includes_zero_and_preserves_its_nine_bit_result_range() {
    let result = KnownBits::transfer(opcode::CLZ, &[KnownBits::top()]);
    assert_eq!(result.zero(), !U256::from(511));
    for value in 0_u64..=256 {
        assert!(result.contains(U256::from(value)));
    }
    let high_bit = KnownBits::new(U256::ZERO, U256::from(1) << 255).unwrap();
    assert_eq!(
        KnownBits::transfer(opcode::CLZ, &[high_bit]).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn invalid_opcodes_or_arity_are_conservative() {
    assert_eq!(
        KnownBits::transfer(0xff, &[KnownBits::top()]),
        KnownBits::top()
    );
    assert_eq!(KnownBits::transfer(opcode::ADD, &[]), KnownBits::top());
    assert_eq!(
        KnownBits::transfer(opcode::ADDMOD, &[KnownBits::top(); 2]),
        KnownBits::top()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn full_word_abstract_arithmetic_covers_concrete_cases(
        a0 in any::<[u8; 32]>(), a1 in any::<[u8; 32]>(),
        b0 in any::<[u8; 32]>(), b1 in any::<[u8; 32]>(),
        c0 in any::<[u8; 32]>(), c1 in any::<[u8; 32]>(),
    ) {
        let a_values = [U256::from_le_bytes(a0), U256::from_le_bytes(a1)];
        let b_values = [U256::from_le_bytes(b0), U256::from_le_bytes(b1)];
        let c_values = [U256::from_le_bytes(c0), U256::from_le_bytes(c1)];
        let a = KnownBits::exact(a_values[0]).join(&KnownBits::exact(a_values[1]));
        let b = KnownBits::exact(b_values[0]).join(&KnownBits::exact(b_values[1]));
        let c = KnownBits::exact(c_values[0]).join(&KnownBits::exact(c_values[1]));
        for op in BINARY_OPS[..23].iter().copied() {
            let result = if matches!(op, opcode::ADDMOD | opcode::MULMOD) {
                KnownBits::transfer(op, &[a, b, c])
            } else {
                KnownBits::transfer(op, &[a, b])
            };
            for a_value in a_values {
                for b_value in b_values {
                    for c_value in c_values {
                        prop_assert!(result.contains(concrete(op, a_value, b_value, c_value)));
                    }
                }
            }
        }
    }

    #[test]
    fn join_is_a_lattice_for_256_bit_masks(
        a in any::<[u8; 32]>(), b in any::<[u8; 32]>(), c in any::<[u8; 32]>(),
    ) {
        let a = KnownBits::exact(U256::from_le_bytes(a));
        let b = KnownBits::exact(U256::from_le_bytes(b));
        let c = KnownBits::exact(U256::from_le_bytes(c));
        prop_assert_eq!(a.join(&a), a);
        prop_assert_eq!(a.join(&b), b.join(&a));
        prop_assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
        prop_assert_eq!(a.join(&KnownBits::top()), KnownBits::top());
        prop_assert_eq!(a.meet(&KnownBits::top()), Some(a));
        prop_assert!(a.join(&b).contains(a.singleton().unwrap()));
        prop_assert!(a.join(&b).contains(b.singleton().unwrap()));
    }
}
