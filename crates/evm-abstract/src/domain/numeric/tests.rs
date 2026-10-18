//! Exact carrier compaction is independent of reduction and candidate queries.

use super::*;

fn finite(values: impl IntoIterator<Item = U256>, interval: Interval) -> NumericValue {
    NumericValue {
        finite: FiniteConstantSet::try_from_values(values.into_iter().collect()).unwrap(),
        interval,
        ..NumericValue::top()
    }
}

fn range(lower: u64, upper: u64) -> Interval {
    Interval::new_unsigned(U256::from(lower), U256::from(upper)).unwrap()
}

fn assert_carrier(original: &NumericValue, drops_finite: bool) -> NumericValue {
    let compact = original.compact_for_retention();
    assert_eq!(compact.bits, original.bits);
    assert_eq!(compact.interval, original.interval);
    assert_eq!(compact.congruence, original.congruence);
    assert_eq!(compact.nonzero, original.nonzero);
    if drops_finite {
        assert!(!original.finite.is_top());
        assert!(compact.finite.is_top());
        assert_eq!(compact.work_size(), 13);
    } else {
        assert_eq!(&compact, original);
    }
    assert_eq!(compact.compact_for_retention(), compact);
    compact
}

#[test]
fn dense_byte_and_half_byte_envelopes_are_exact_over_the_full_word_universe() {
    for upper in [127u64, 255] {
        let original = finite((0..=upper).map(U256::from), range(0, upper));
        let compact = assert_carrier(&original, true);
        let values = original.constants().unwrap();
        // These two invariants prove R subset F for every U256, rather than
        // treating the boundary samples below as a full-word exhaustive oracle.
        let (lo, hi) = compact.interval.unsigned_bounds();
        assert!(*values.first().unwrap() <= lo && hi <= *values.last().unwrap());
        assert_eq!(hi - lo + U256::from(1), U256::from(values.len()));
        for candidate in (0..=upper + 1)
            .map(U256::from)
            .chain([U256::from(1) << 255, U256::MAX])
        {
            assert_eq!(original.contains(candidate), compact.contains(candidate));
        }
        assert!(!compact.is_top());
        assert_eq!(original.work_size(), usize::try_from(upper).unwrap() + 13);
    }
}

#[test]
fn exhaustive_three_bit_sets_intervals_and_independent_constraints_keep_membership() {
    let congruences = [
        Congruence::top(),
        Congruence::new(U256::from(2), U256::ZERO).unwrap(),
        Congruence::new(U256::from(3), U256::from(1)).unwrap(),
    ];
    let masks = [
        KnownBits::top(),
        KnownBits::new(U256::from(1), U256::ZERO).unwrap(),
        KnownBits::new(U256::ZERO, U256::from(2)).unwrap(),
    ];
    for subset in 1u16..256 {
        let values = (0u64..8)
            .filter(|bit| subset & (1 << bit) != 0)
            .map(U256::from)
            .collect::<BTreeSet<_>>();
        let min = *values.first().unwrap();
        let max = *values.last().unwrap();
        for lower in 0..8 {
            for upper in lower..8 {
                for bits in masks {
                    for congruence in &congruences {
                        for nonzero in [false, true] {
                            let original = NumericValue {
                                finite: FiniteConstantSet::try_from_values(values.clone()).unwrap(),
                                bits,
                                interval: range(lower, upper),
                                congruence: congruence.clone(),
                                nonzero,
                            };
                            // Independent small integer oracle: a dense set
                            // includes each of its integer interior positions.
                            let dense = (0..8)
                                .map(U256::from)
                                .filter(|candidate| min <= *candidate && *candidate <= max)
                                .all(|candidate| values.contains(&candidate));
                            let drops = values.len() > 1
                                && dense
                                && min <= U256::from(lower)
                                && U256::from(upper) <= max;
                            let compact = assert_carrier(&original, drops);
                            for candidate in (0u64..=8).map(U256::from).chain([U256::MAX]) {
                                assert_eq!(
                                    original.contains(candidate),
                                    compact.contains(candidate)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn holes_and_unenclosed_or_partly_overlapping_intervals_fail_closed() {
    let mut holes = finite([0u64, 1, 2, 3, 8, 9, 10, 11].map(U256::from), range(0, 11));
    let unchanged = assert_carrier(&holes, false);
    assert_eq!(unchanged.constants().unwrap().len(), 8);
    assert!(!unchanged.contains(U256::from(4)));
    // Even an exactly equivalent known-bits cube is outside this predicate.
    holes.bits = KnownBits::from_values(holes.constants().unwrap());
    assert_carrier(&holes, false);
    for interval in [
        range(0, 15),
        range(0, 7),
        range(8, 15),
        range(20, 30),
        Interval::top(),
    ] {
        assert_carrier(&finite((4u64..=11).map(U256::from), interval), false);
    }
    // A strict interior envelope is sufficient even when it makes some of F
    // infeasible. No finite filtering or change to any other field is needed.
    assert_carrier(&finite((4u64..=11).map(U256::from), range(6, 9)), true);
}

#[test]
fn signed_wrap_and_word_span_overflow_are_not_dense_linear_ranges() {
    let sign = U256::from(1) << 255;
    let signed_wrap = Interval::new_signed(sign - U256::from(2), sign + U256::from(2)).unwrap();
    assert_eq!(signed_wrap.segments().len(), 2);
    assert_carrier(&finite((0u64..=255).map(U256::from), signed_wrap), false);
    assert_carrier(&finite([U256::ZERO, U256::MAX], Interval::top()), false);
    assert_carrier(
        &finite([U256::ZERO, U256::from(1), U256::MAX], Interval::top()),
        false,
    );
    // A dense range across the sign boundary is still an unsigned closed range.
    let lower = sign - U256::from(2);
    let upper = sign + U256::from(2);
    let interval = Interval::new_unsigned(lower, upper).unwrap();
    let original = finite((0u64..5).map(|offset| lower + U256::from(offset)), interval);
    let compact = assert_carrier(&original, true);
    for candidate in [
        U256::ZERO,
        lower - U256::from(1),
        lower,
        sign,
        upper,
        upper + U256::from(1),
        U256::MAX,
    ] {
        assert_eq!(original.contains(candidate), compact.contains(candidate));
    }
    let lower = U256::MAX - U256::from(3);
    assert_carrier(
        &finite(
            (0u64..4).map(|offset| lower + U256::from(offset)),
            Interval::new_unsigned(lower, U256::MAX).unwrap(),
        ),
        true,
    );
}

#[test]
fn masks_nonzero_and_non_coprime_crt_results_are_preserved_without_solving() {
    let left = Congruence::new(U256::from(6), U256::from(2)).unwrap();
    let compatible = Congruence::new(U256::from(8), U256::from(2)).unwrap();
    let incompatible = Congruence::new(U256::from(8), U256::from(3)).unwrap();
    assert!(left.meet(&incompatible).is_none());
    let crt = left.meet(&compatible).unwrap();
    let original = NumericValue {
        bits: KnownBits::new(U256::MAX << 8usize, U256::from(2)).unwrap(),
        congruence: crt.clone(),
        nonzero: true,
        ..finite((0u64..=255).map(U256::from), range(0, 255))
    };
    let compact = assert_carrier(&original, true);
    assert_eq!(compact.congruence, crt);
    for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
        assert_eq!(original.contains(candidate), compact.contains(candidate));
    }
    assert!(compact.contains(U256::from(2)));
    assert!(!compact.contains(U256::from(8)));
    assert!(!compact.contains(U256::ZERO));
}

#[test]
fn top_and_singleton_finite_components_keep_their_existing_carriers() {
    for original in [
        NumericValue::top(),
        NumericValue::unknown_byte(),
        NumericValue::constant(U256::ZERO),
        NumericValue::constant(U256::MAX),
    ] {
        assert_carrier(&original, false);
    }
    let singleton_finite = finite([U256::from(7)], Interval::top());
    assert_carrier(&singleton_finite, false);
}

#[test]
fn profile_projection_and_later_reduction_do_not_change_the_carrier_gamma_proof() {
    use crate::domain::{AbstractValue, Domain, DomainSpec, Profile};
    use std::num::NonZeroUsize;

    let original = NumericValue {
        finite: FiniteConstantSet::try_from_values((0u64..=255).map(U256::from).collect()).unwrap(),
        ..NumericValue::unknown_byte()
    };
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let domain = Domain::from_spec(DomainSpec::new(
            profile,
            NonZeroUsize::new(256).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(1024).unwrap(),
        ));
        let projected = domain.project(&AbstractValue::from_numeric(original.clone()));
        let carrier = assert_carrier(projected.numeric(), true);
        // Subsequent projection/reduction is allowed to enumerate normally.
        // Neither result is fed back into the executable binding constructor.
        let value = AbstractValue::from_numeric(carrier.clone());
        let projected_again = domain.project(&value);
        if profile == Profile::Product {
            assert!(projected_again.constants().is_none());
        } else {
            assert_eq!(projected_again.constants(), original.constants());
        }
        let reduced = domain.reduce(&value).value;
        assert_eq!(reduced.constants(), original.constants());
        for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
            assert_eq!(projected.contains(candidate), carrier.contains(candidate));
            assert_eq!(carrier.contains(candidate), reduced.contains(candidate));
        }
        assert!(carrier.constants().is_none());
    }
}
