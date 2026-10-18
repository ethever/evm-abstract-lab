//! Independent finite-universe controls; no query/reduction oracle for Gamma.

use super::*;
use crate::domain::{
    AbstractValue, Domain, DomainSpec, Profile, congruence::Congruence, identity::Symbol,
    interval::Interval, known_bits::KnownBits, provenance::Origin,
};
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

fn range(lower: u64, upper: u64) -> Interval {
    Interval::new_unsigned(U256::from(lower), U256::from(upper)).unwrap()
}

fn assert_remaining_components(actual: &NumericValue, original: &NumericValue) {
    assert_eq!(actual.bits, original.bits);
    assert_eq!(actual.interval, original.interval);
    assert_eq!(actual.congruence, original.congruence);
    assert_eq!(actual.nonzero, original.nonzero);
}

#[test]
fn complete_envelopes_match_exhaustive_three_bit_constraints_including_holes() {
    let sign = U256::from(1) << 255;
    for lower in 0u64..8 {
        for upper in lower..8 {
            for (signed_lower, signed_upper) in [(0u64, 7u64), (2, 5)] {
                let Some(interval) = range(lower, upper).meet(
                    &Interval::new_signed(
                        sign + U256::from(signed_lower),
                        sign + U256::from(signed_upper),
                    )
                    .unwrap(),
                ) else {
                    continue;
                };
                for zero in 0u64..8 {
                    for one in 0u64..8 {
                        if zero & one != 0 {
                            continue;
                        }
                        for modulus in 1u64..=4 {
                            for residue in 0..modulus {
                                for nonzero in [false, true] {
                                    let current = NumericValue {
                                        finite: FiniteConstantSet::top(),
                                        bits: KnownBits::new(U256::from(zero), U256::from(one))
                                            .unwrap(),
                                        interval,
                                        congruence: Congruence::new(
                                            U256::from(modulus),
                                            U256::from(residue),
                                        )
                                        .unwrap(),
                                        nonzero,
                                    };
                                    // Plain small-integer predicates do not call
                                    // NumericValue::contains or candidate queries.
                                    let expected: BTreeSet<_> = (0u64..8)
                                        .filter(|x| {
                                            lower <= *x
                                                && *x <= upper
                                                && signed_lower <= *x
                                                && *x <= signed_upper
                                                && x & zero == 0
                                                && x & one == one
                                                && x % modulus == residue
                                                && (!nonzero || *x != 0)
                                        })
                                        .map(U256::from)
                                        .collect();
                                    let plan = current.complete_enumeration(8).unwrap();
                                    let (lo, hi) = current.interval.unsigned_bounds();
                                    assert_eq!(U256::from(plan.count()), hi - lo + U256::from(1));
                                    let mut checkpoints = 0;
                                    let built = plan.build(|| {
                                        checkpoints += 1;
                                        true
                                    });
                                    assert_eq!(checkpoints, plan.count() + 1);
                                    assert_eq!(built.is_none(), expected.is_empty());
                                    if let Some(built) = built {
                                        assert_eq!(built.constants(), Some(&expected));
                                        assert_remaining_components(&built, &current);
                                        for x in (0u64..=8).map(U256::from).chain([U256::MAX]) {
                                            assert_eq!(built.contains(x), expected.contains(&x));
                                        }
                                    }
                                    assert!(current.constants().is_none());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn high_word_signed_and_non_power_of_two_constraints_remain_current() {
    let first = U256::MAX - U256::from(7);
    let interval = Interval::new_unsigned(first, U256::MAX)
        .unwrap()
        .meet(
            &Interval::new_signed(
                first ^ (U256::from(1) << 255),
                (U256::MAX - U256::from(2)) ^ (U256::from(1) << 255),
            )
            .unwrap(),
        )
        .unwrap();
    let current = NumericValue {
        interval,
        bits: KnownBits::new(U256::from(2), U256::ZERO).unwrap(),
        congruence: Congruence::new(U256::from(3), U256::from(1)).unwrap(),
        nonzero: true,
        ..NumericValue::top()
    };
    let expected: BTreeSet<_> = (0u64..8)
        .filter(|delta| {
            // 2^256 - 1 is 0 modulo 3 and has every low bit set.
            *delta >= 2 && delta % 3 == 2 && delta & 2 != 0
        })
        .map(|delta| U256::MAX - U256::from(delta))
        .collect();
    let built = current
        .complete_enumeration(8)
        .unwrap()
        .build(|| true)
        .unwrap();
    assert_eq!(built.constants(), Some(&expected));
    assert_remaining_components(&built, &current);
}

#[test]
fn span_overflow_wide_intervals_and_existing_finite_components_are_rejected_without_visits() {
    assert!(
        NumericValue::top()
            .complete_enumeration(usize::MAX)
            .is_none()
    );
    assert!(
        NumericValue::unknown_byte()
            .complete_enumeration(255)
            .is_none()
    );
    assert_eq!(
        NumericValue::unknown_byte()
            .complete_enumeration(256)
            .unwrap()
            .count(),
        256
    );
    let too_wide = NumericValue::unsigned_range(U256::ZERO, U256::from(usize::MAX)).unwrap();
    assert!(too_wide.complete_enumeration(usize::MAX).is_none());
    let signed_wrap = NumericValue {
        interval: Interval::new_signed(
            (U256::from(1) << 255) - U256::from(2),
            (U256::from(1) << 255) + U256::from(2),
        )
        .unwrap(),
        ..NumericValue::top()
    };
    assert!(signed_wrap.complete_enumeration(8).is_none());
    let finite = NumericValue {
        finite: FiniteConstantSet::try_from_values([U256::ZERO, U256::from(7)].into()).unwrap(),
        bits: KnownBits::exact(U256::from(7)),
        interval: range(0, 7),
        ..NumericValue::top()
    };
    let frozen = finite.clone();
    assert!(finite.complete_enumeration(8).is_none());
    assert_eq!(finite, frozen);
}

#[test]
fn empty_and_interrupted_construction_never_manufacture_a_numeric_value() {
    let empty = NumericValue {
        interval: range(0, 0),
        nonzero: true,
        ..NumericValue::top()
    };
    let plan = empty.complete_enumeration(1).unwrap();
    assert_eq!(plan.count(), 1);
    assert!(plan.build(|| true).is_none());
    let value = NumericValue::unknown_byte();
    let plan = value.complete_enumeration(256).unwrap();
    for stop in [1, 2, 128, 256, 257] {
        let mut checkpoints = 0;
        assert!(
            plan.build(|| {
                checkpoints += 1;
                checkpoints < stop
            })
            .is_none()
        );
        assert_eq!(checkpoints, stop);
        assert!(value.constants().is_none());
    }
    let complete = plan.build(|| true).unwrap();
    assert_eq!(complete.constants().unwrap().len(), 256);
}

#[test]
fn full_finite_apply_outputs_match_independent_small_gamma_operands() {
    let domain = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(16).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ));
    for (zero, one, modulus, residue, nonzero) in [
        (0u64, 0u64, 1u64, 0u64, false),
        (2, 0, 1, 0, true),
        (0, 1, 3, 1, false),
        (0, 0, 3, 2, true),
    ] {
        let current = NumericValue {
            bits: KnownBits::new(U256::from(zero), U256::from(one)).unwrap(),
            interval: range(0, 7),
            congruence: Congruence::new(U256::from(modulus), U256::from(residue)).unwrap(),
            nonzero,
            ..NumericValue::top()
        };
        let expected: BTreeSet<_> = (0u64..8)
            .filter(|x| {
                x & zero == 0 && x & one == one && x % modulus == residue && (!nonzero || *x != 0)
            })
            .map(U256::from)
            .collect();
        let exact = NumericValue {
            finite: FiniteConstantSet::try_from_values(expected).unwrap(),
            ..current.clone()
        };
        let base = AbstractValue::from_numeric(current.clone())
            .with_origin(Origin::Calldata)
            .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(871_021))
            .with_identity(871_022, 1)
            .with_symbolic_limit();
        let actual = base.clone().with_numeric(
            current
                .complete_enumeration(8)
                .unwrap()
                .build(|| true)
                .unwrap(),
        );
        let expected = base.with_numeric(exact);
        for op in [
            opcode::ADD,
            opcode::AND,
            opcode::XOR,
            opcode::SHL,
            opcode::BYTE,
        ] {
            let actual = domain.apply(
                op,
                &[actual.clone(), AbstractValue::constant(U256::from(3))],
            );
            let expected = domain.apply(
                op,
                &[expected.clone(), AbstractValue::constant(U256::from(3))],
            );
            assert_eq!(actual, expected);
            assert_eq!(actual.identity(), expected.identity());
            assert_eq!(
                actual.symbolic_limit_reached(),
                expected.symbolic_limit_reached()
            );
        }
    }
}
