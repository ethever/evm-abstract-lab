//! Exact carrier compaction is independent of reduction and candidate queries.

use crate::domain::NumericValue;

use super::*;

#[test]
fn sparse_native_json_omits_only_top_components_and_keeps_each_actual_constraint() {
    assert_eq!(
        serde_json::to_value(NumericValue::top()).unwrap(),
        serde_json::json!({})
    );
    for (key, numeric) in [
        (
            "constants",
            NumericValue {
                finite: FiniteConstantSet::try_from_values(
                    [U256::ZERO, U256::from(7)].into_iter().collect(),
                )
                .unwrap(),
                ..NumericValue::top()
            },
        ),
        (
            "known_bits",
            NumericValue {
                bits: KnownBits::exact(U256::from(7)),
                ..NumericValue::top()
            },
        ),
        (
            "interval",
            NumericValue {
                interval: range(3, 9),
                ..NumericValue::top()
            },
        ),
        (
            "congruence",
            NumericValue {
                congruence: Congruence::new(U256::from(3), U256::ONE).unwrap(),
                ..NumericValue::top()
            },
        ),
        (
            "nonzero",
            NumericValue {
                nonzero: true,
                ..NumericValue::top()
            },
        ),
    ] {
        let json = serde_json::to_value(&numeric).unwrap();
        assert_eq!(
            json.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [key]
        );
        assert!(!json[key].is_null());
        let value = crate::domain::EvmValue::from_numeric(numeric);
        assert_eq!(serde_json::to_value(value).unwrap()["numeric"], json);
    }
}

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
        let restored = compact.restore_redundant_finite_from(original);
        assert_eq!(&restored, original);
        assert!(std::ptr::eq(
            restored.constants().unwrap(),
            original.constants().unwrap()
        ));
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
fn product_projection_and_later_reduction_do_not_change_the_carrier_gamma_proof() {
    use crate::domain::{DomainSpec, EvmValue, ProductAlgebra, ValueAlgebra};
    use std::num::NonZeroUsize;

    let original = NumericValue {
        finite: FiniteConstantSet::try_from_values((0u64..=255).map(U256::from).collect()).unwrap(),
        ..NumericValue::unknown_byte()
    };
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(256).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
    )));
    let projected = domain.project(&EvmValue::<NumericValue>::from_numeric(original.clone()));
    let carrier = assert_carrier(projected.numeric(), true);
    // Subsequent projection/reduction is allowed to enumerate normally.
    // Neither result is fed back into the executable binding constructor.
    let value = EvmValue::<NumericValue>::from_numeric(carrier.clone());
    let projected_again = domain.project(&value);
    assert!(projected_again.constants().is_none());
    let reduced = domain.reduce(&value).value;
    assert_eq!(reduced.constants(), original.constants());
    for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
        assert_eq!(projected.contains(candidate), carrier.contains(candidate));
        assert_eq!(carrier.contains(candidate), reduced.contains(candidate));
    }
    assert!(carrier.constants().is_none());
}

#[test]
fn restoration_uses_only_current_constraints_and_shares_only_the_old_finite_allocation() {
    // Deliberately stale old R must contribute no fields to the current result.
    let previous = NumericValue {
        bits: KnownBits::exact(U256::from(200)),
        congruence: Congruence::exact(U256::from(200)),
        nonzero: false,
        ..finite((0u64..=255).map(U256::from), range(200, 255))
    };
    let current = NumericValue {
        bits: KnownBits::new(U256::from(1), U256::from(2)).unwrap(),
        interval: range(4, 31),
        congruence: Congruence::new(U256::from(3), U256::from(1)).unwrap(),
        nonzero: true,
        ..NumericValue::top()
    };
    let restored = current.restore_redundant_finite_from(&previous);
    assert_eq!(restored.bits, current.bits);
    assert_eq!(restored.interval, current.interval);
    assert_eq!(restored.congruence, current.congruence);
    assert_eq!(restored.nonzero, current.nonzero);
    assert!(std::ptr::eq(
        restored.constants().unwrap(),
        previous.constants().unwrap()
    ));
    assert_eq!(restored.restore_redundant_finite_from(&previous), restored);
    assert_eq!(restored.compact_for_retention(), current);
    for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
        assert_eq!(restored.contains(candidate), current.contains(candidate));
    }
}

#[test]
fn restoration_refuses_stale_wide_bounds_holes_overflow_and_every_current_finite() {
    let previous = finite((0u64..=255).map(U256::from), range(0, 255));
    for interval in [range(0, 511), range(128, 511), Interval::top()] {
        let current = NumericValue {
            interval,
            ..NumericValue::top()
        };
        assert_eq!(current.restore_redundant_finite_from(&previous), current);
    }
    for current in [
        finite([1u64, 3, 5].map(U256::from), range(0, 255)),
        finite((4u64..=7).map(U256::from), range(4, 7)),
        NumericValue::constant(U256::from(7)),
    ] {
        let restored = current.restore_redundant_finite_from(&previous);
        assert_eq!(restored, current);
        assert!(std::ptr::eq(
            restored.constants().unwrap(),
            current.constants().unwrap()
        ));
    }
    let current = NumericValue::unsigned_range(U256::ZERO, U256::from(11)).unwrap();
    for previous in [
        finite([0u64, 1, 2, 3, 8, 9, 10, 11].map(U256::from), range(0, 11)),
        finite([U256::ZERO, U256::MAX], Interval::top()),
        NumericValue::constant(U256::ZERO),
        NumericValue::top(),
    ] {
        assert_eq!(current.restore_redundant_finite_from(&previous), current);
    }
}

#[test]
fn negative_singleton_queries_pay_the_fixed_native_path_without_membership_or_division() {
    use crate::{
        domain::{EvmValue, WordElement, summary},
        resource::WorkBudget,
    };
    let _scope = SINGLETON_CALLS.scope();
    let mut values = vec![
        NumericValue::top(),
        NumericValue::unknown_byte(),
        NumericValue::unknown_address(),
    ];
    values.push(NumericValue {
        finite: FiniteConstantSet::try_from_values((0u64..512).map(U256::from).collect()).unwrap(),
        bits: KnownBits::new(U256::MAX << 9, U256::ZERO).unwrap(),
        interval: Interval::new_unsigned(U256::ZERO, U256::from(511)).unwrap(),
        congruence: crate::domain::congruence::Congruence::new(U256::from(3), U256::ONE).unwrap(),
        nonzero: true,
    });
    for numeric in values {
        let value = EvmValue::from_numeric(numeric);
        let fee = value.numeric().singleton_work();
        SINGLETON_CALLS.set((0, 0));
        let mut exact = WorkBudget::new(fee);
        assert_eq!(summary::singleton(&value, &mut exact).unwrap(), None);
        assert_eq!(exact.used(), fee);
        assert_eq!(SINGLETON_CALLS.get(), (1, 0));
        SINGLETON_CALLS.set((0, 0));
        let mut denied = WorkBudget::new(fee - 1);
        assert!(summary::singleton(&value, &mut denied).is_err());
        assert_eq!(SINGLETON_CALLS.get(), (0, 0));
        assert!(denied.used() < fee);
    }
    let candidate = EvmValue::from_numeric(NumericValue {
        finite: FiniteConstantSet::try_from_values(
            [U256::ONE, U256::from(7)].into_iter().collect(),
        )
        .unwrap(),
        bits: KnownBits::exact(U256::from(7)),
        interval: Interval::new_unsigned(U256::ZERO, U256::from(9)).unwrap(),
        congruence: crate::domain::congruence::Congruence::new(U256::from(3), U256::ONE).unwrap(),
        nonzero: true,
    });
    let fee = candidate.numeric().singleton_work();
    assert!(fee >= 4096);
    SINGLETON_CALLS.set((0, 0));
    let mut exact = WorkBudget::new(fee);
    assert_eq!(
        summary::singleton(&candidate, &mut exact).unwrap(),
        Some(U256::from(7))
    );
    assert_eq!(exact.used(), fee);
    assert_eq!(SINGLETON_CALLS.get(), (1, 1));
}

#[test]
fn canonical_literal_queries_keep_every_native_component_and_skip_redundant_membership() {
    use crate::{
        domain::{EvmValue, WordElement, summary},
        resource::WorkBudget,
    };
    let _scope = SINGLETON_CALLS.scope();
    for word in [
        U256::ZERO,
        U256::ONE,
        U256::from(170),
        U256::ONE << 255,
        U256::MAX,
    ] {
        let value = EvmValue::from_numeric(NumericValue::constant(word));
        let fee = value.numeric().singleton_work();
        assert_eq!(fee, 256);
        SINGLETON_CALLS.set((0, 0));
        let mut paid = WorkBudget::new(fee);
        assert_eq!(summary::singleton(&value, &mut paid).unwrap(), Some(word));
        assert_eq!(paid.used(), fee);
        assert_eq!(SINGLETON_CALLS.get(), (1, 0));
        SINGLETON_CALLS.set((0, 0));
        let mut denied = WorkBudget::new(fee - 1);
        assert!(summary::singleton(&value, &mut denied).is_err());
        assert_eq!(SINGLETON_CALLS.get(), (0, 0));
    }
    for numeric in [
        NumericValue {
            nonzero: true,
            ..NumericValue::constant(U256::ZERO)
        },
        NumericValue {
            bits: KnownBits::exact(U256::ONE),
            ..NumericValue::constant(U256::ZERO)
        },
        NumericValue {
            interval: Interval::exact(U256::ONE),
            ..NumericValue::constant(U256::ZERO)
        },
        NumericValue {
            congruence: Congruence::exact(U256::ONE),
            ..NumericValue::constant(U256::ZERO)
        },
    ] {
        assert!(numeric.canonical_literal().is_none());
        let value = EvmValue::from_numeric(numeric);
        let fee = value.numeric().singleton_work();
        assert!(fee > 256);
        SINGLETON_CALLS.set((0, 0));
        let mut paid = WorkBudget::new(fee);
        assert_eq!(summary::singleton(&value, &mut paid).unwrap(), None);
        assert_eq!(SINGLETON_CALLS.get(), (1, 1));
    }
}
