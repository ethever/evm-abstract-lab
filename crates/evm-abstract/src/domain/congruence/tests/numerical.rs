use super::super::{Congruence, Kind, gcd};
use crate::domain::evaluate;
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

// Frozen pre-optimization Euclid, normalization and join fold: the oracle must
// not call the optimized gcd/new/join/from_values dependency chain.
fn old_gcd(mut a: U256, mut b: U256) -> U256 {
    while b != U256::ZERO {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn old_new(modulus: U256, residue: U256) -> Option<Congruence> {
    if modulus == U256::ZERO {
        return None;
    }
    if modulus == U256::ONE {
        return Some(Congruence::top());
    }
    let residue = residue % modulus;
    if residue.checked_add(modulus).is_none() {
        Some(Congruence::exact(residue))
    } else {
        Some(Congruence(Kind::Modulo { modulus, residue }))
    }
}

fn old_join(left: &Congruence, right: &Congruence) -> Congruence {
    if left == right {
        return left.clone();
    }
    if left.is_top() || right.is_top() {
        return Congruence::top();
    }
    let (left_modulus, left_residue) = left.integer_class();
    let (right_modulus, right_residue) = right.integer_class();
    let difference = if left_residue >= right_residue {
        left_residue - right_residue
    } else {
        right_residue - left_residue
    };
    let modulus = old_gcd(old_gcd(left_modulus, right_modulus), difference);
    if modulus == U256::ZERO {
        Congruence::exact(left_residue)
    } else {
        old_new(modulus, left_residue).unwrap()
    }
}

fn old_from_values(values: &BTreeSet<U256>) -> Congruence {
    let mut values = values.iter().copied();
    let Some(first) = values.next() else {
        return Congruence::top();
    };
    values.fold(Congruence::exact(first), |left, value| {
        old_join(&left, &Congruence::exact(value))
    })
}

fn next_word(state: &mut u64) -> U256 {
    U256::from_limbs(std::array::from_fn(|_| {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }))
}

fn assert_gcd_matches_old(a: U256, b: U256) {
    let expected = old_gcd(a, b);
    assert_eq!(gcd(a, b), expected, "a={a}, b={b}");
    assert_eq!(gcd(b, a), expected, "a={b}, b={a}");
}

fn assert_hull_matches_old(values: &BTreeSet<U256>) {
    let actual = Congruence::from_values(values);
    assert_eq!(actual, old_from_values(values), "values={values:?}");
    assert!(values.iter().all(|value| actual.contains(*value)));
}

#[test]
fn gcd_zero_powers_and_limb_edges_match_old_euclid() {
    let samples = [
        U256::ZERO,
        U256::ONE,
        U256::MAX,
        U256::MAX - U256::ONE,
        U256::from_limbs([0, 3, 0, 0]),
        U256::from_limbs([0, 0, 3, 1]),
        U256::from_limbs([0, 0, 0, 3]),
        U256::from_limbs([u64::MAX, 1, u64::MAX, 1]),
    ];
    for a in samples {
        for b in samples {
            assert_gcd_matches_old(a, b);
        }
    }
    for bit in 0..256 {
        let power = U256::ONE << bit;
        for other_bit in [0, 1, 63, 64, 65, 127, 128, 129, 191, 192, 193, 254, 255] {
            assert_gcd_matches_old(power, U256::ONE << other_bit);
        }
        for other in samples
            .into_iter()
            .chain([power, power - U256::ONE, power | U256::ONE])
        {
            assert_gcd_matches_old(power, other);
        }
    }
}

#[test]
fn gcd_generated_wide_pairs_match_old_euclid() {
    let mut state = 0x9e37_79b9_7f4a_7c15;
    for case in 0..512 {
        let a = next_word(&mut state);
        let b = next_word(&mut state);
        assert_gcd_matches_old(a, b);
        assert_gcd_matches_old(a << (case % 256), b << (case % 256));
        // Exact products retain a generated wide common factor, so random
        // coprime inputs alone cannot hide a faulty general-case fallback.
        let common = next_word(&mut state) >> 128;
        assert_gcd_matches_old((a >> 128) * common, (b >> 128) * common);
    }
}

#[test]
fn normalization_matches_old_remainder_and_singleton_canonicalization() {
    let moduli = [
        U256::ZERO,
        U256::ONE,
        U256::from(3),
        U256::from(6),
        U256::MAX,
        (U256::ONE << 64) - U256::ONE,
        (U256::ONE << 64) + U256::ONE,
        (U256::ONE << 128) + U256::from(3),
        (U256::ONE << 255) - U256::ONE,
        (U256::ONE << 255) + U256::ONE,
    ];
    let mut state = 0xd1b5_4a32_d192_ed03;
    for modulus in moduli
        .into_iter()
        .chain((0..256).map(|bit| U256::ONE << bit))
    {
        for residue in [
            U256::ZERO,
            U256::ONE,
            U256::MAX,
            U256::MAX - U256::ONE,
            modulus.wrapping_sub(U256::ONE),
            modulus,
            modulus.wrapping_add(U256::ONE),
            next_word(&mut state),
        ] {
            assert_eq!(
                Congruence::new(modulus, residue),
                old_new(modulus, residue),
                "modulus={modulus}, residue={residue}"
            );
        }
    }
    for _ in 0..512 {
        let modulus = next_word(&mut state);
        let residue = next_word(&mut state);
        assert_eq!(
            Congruence::new(modulus, residue),
            old_new(modulus, residue),
            "modulus={modulus}, residue={residue}"
        );
    }
}

#[test]
fn hull_empty_singleton_sparse_and_word_edges_match_old_fold() {
    let half = U256::ONE << 255;
    let high = U256::ONE << 200;
    for values in [
        BTreeSet::new(),
        BTreeSet::from([U256::ZERO]),
        BTreeSet::from([U256::MAX]),
        BTreeSet::from([U256::ZERO, U256::MAX]),
        BTreeSet::from([U256::ONE, U256::MAX]),
        BTreeSet::from([U256::ZERO, half]),
        BTreeSet::from([half - U256::ONE, U256::MAX]),
        BTreeSet::from([3, 9, 15].map(U256::from)),
        BTreeSet::from([5, 17, 35, 65].map(U256::from)),
        BTreeSet::from([17, 23, 32].map(|offset| high + U256::from(offset))),
        BTreeSet::from([12, 6, 0].map(|offset| U256::MAX - U256::from(offset))),
    ] {
        assert_hull_matches_old(&values);
    }
}

#[test]
fn hull_generated_wide_sets_match_old_fold() {
    let mut state = 0xa076_1d64_78bd_642f;
    for case in 0..256 {
        let arbitrary = (0..case % 33).map(|_| next_word(&mut state)).collect();
        assert_hull_matches_old(&arbitrary);

        // No wrap: first < 2^255 and 28 * spacing < 2^253. These sparse
        // progressions keep a non-power modulus and change the gcd late.
        let first = next_word(&mut state) >> 1;
        let spacing = (next_word(&mut state) >> 8) | U256::from(3);
        let spaced = [0, 6, 15, 28]
            .map(|factor| first + spacing * U256::from(factor))
            .into();
        assert_hull_matches_old(&spaced);
        assert_eq!(
            Congruence::from_values(&spaced),
            old_new(spacing, first).unwrap()
        );

        let first = next_word(&mut state);
        let spacing = next_word(&mut state);
        let wrapped = [0, 1, 3, 8, 21]
            .map(|factor| first.wrapping_add(spacing.wrapping_mul(U256::from(factor))))
            .into();
        assert_hull_matches_old(&wrapped);
    }
}

#[test]
fn all_byte_shift_covers_and_sparse_wrapped_shl_hulls_match_old_fold() {
    let bytes = (0..=u8::MAX).map(U256::from).collect::<BTreeSet<_>>();
    let sparse = BTreeSet::from([
        U256::ZERO,
        U256::ONE,
        U256::from(3),
        U256::from(255),
        U256::ONE << 63,
        (U256::ONE << 64) + U256::ONE,
        (U256::ONE << 127) + U256::from(3),
        (U256::ONE << 128) + (U256::ONE << 65),
        (U256::ONE << 200) + U256::from(17),
        U256::ONE << 255,
        U256::MAX - U256::ONE,
        U256::MAX,
    ]);
    let odd_spaced = BTreeSet::from([
        U256::from(3),
        U256::from(9),
        U256::from(15),
        U256::MAX - U256::from(6),
        U256::MAX,
    ]);
    for shift in (0..=256_u64).map(U256::from).chain([U256::MAX]) {
        for source in [&bytes, &sparse, &odd_spaced] {
            let shifted = source
                .iter()
                .map(|value| evaluate(opcode::SHL, shift, *value, U256::ZERO))
                .collect();
            assert_hull_matches_old(&shifted);
        }
        let shifted_bytes = bytes
            .iter()
            .map(|value| evaluate(opcode::SHL, shift, *value, U256::ZERO))
            .collect::<BTreeSet<_>>();
        let (expected, cardinality) = if shift >= U256::from(256) {
            (Congruence::exact(U256::ZERO), 1)
        } else if shift == U256::ZERO {
            (Congruence::top(), 256)
        } else {
            let shift = shift.to::<usize>();
            (
                Congruence(Kind::Modulo {
                    modulus: U256::ONE << shift,
                    residue: U256::ZERO,
                }),
                if shift <= 248 {
                    256
                } else {
                    1 << (256 - shift)
                },
            )
        };
        assert_eq!(shifted_bytes.len(), cardinality, "shift={shift}");
        assert_eq!(
            Congruence::from_values(&shifted_bytes),
            expected,
            "shift={shift}"
        );
    }
}
