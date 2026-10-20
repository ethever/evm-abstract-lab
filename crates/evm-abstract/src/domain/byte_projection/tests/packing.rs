//! Differential default-policy proof cases; no timing or contract fixture.
use super::*;
use crate::domain::{EvmNumericAlgebra, EvmValue, ReductionStatus, ValueAlgebra};

fn policy(profile: Profile, capacity: usize, rounds: usize, facts: usize) -> Domain {
    Domain::from_spec(
        DomainSpec::new(
            profile,
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(rounds).unwrap(),
            NonZeroUsize::new(facts).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: true,
            max_constraints: 2048,
            max_nodes: 16384,
            max_depth: 256,
        }),
    )
}

fn cube(free: u8, one: u8, finite: bool) -> NumericValue {
    assert_eq!(free & one, 0);
    let mut value = NumericValue::unknown_byte();
    value.bits = KnownBits::new(!U256::from(free | one), U256::from(one)).unwrap();
    if finite {
        value.finite = FiniteConstantSet::try_from_values(
            (0..=255u64)
                .map(U256::from)
                .filter(|v| value.bits.contains(*v))
                .collect(),
        )
        .unwrap();
    }
    value
}

fn captured() -> [NumericValue; 32] {
    let mut bytes = std::array::from_fn(|_| NumericValue::unknown_byte());
    bytes[31] = cube(0xe0, 0, true);
    bytes[31].interval = Interval::new_unsigned(U256::ZERO, U256::from(224)).unwrap();
    bytes[31].congruence = Congruence::new(U256::from(32), U256::ZERO).unwrap();
    bytes
}

/// The original ordered injected scalar fold, retaining every operation and its
/// full detailed status. No packing helper or observation roundtrip is involved.
fn ordinary(domain: Domain, bytes: &[NumericValue; 32]) -> NumericValue {
    let algebra = ValueAlgebra::new(domain);
    let mut word = EvmValue::constant(U256::ZERO);
    for (i, byte) in bytes.iter().enumerate() {
        let shifted = algebra.apply_detailed(
            opcode::SHL,
            &[
                EvmValue::constant(U256::from((31 - i) * 8)),
                EvmValue::from_numeric(byte.clone()),
            ],
        );
        assert_eq!(shifted.status, ReductionStatus::Stable, "SHL lane {i}");
        // Engine::apply deliberately returns only the numeric carrier.
        let next = algebra.apply_detailed(
            opcode::OR,
            &[
                word,
                EvmValue::from_numeric(shifted.value.numeric().clone()),
            ],
        );
        assert_eq!(next.status, ReductionStatus::Stable, "OR lane {i}");
        word = EvmValue::from_numeric(next.value.numeric().clone());
    }
    word.numeric().clone()
}

fn equivalent(domain: Domain, bytes: &[NumericValue; 32]) -> NumericValue {
    let refs = std::array::from_fn(|i| &bytes[i]);
    let result = domain.pack_word_numeric(&refs).expect("certified cubes");
    assert_eq!(result, ordinary(domain, bytes), "spec={:?}", domain.spec());
    assert_eq!(
        domain.pack_word_work(&refs),
        ValueAlgebra::new(&domain).borrowed().pack_word_work(&refs)
    );
    assert_eq!(
        Some(result.clone()),
        ValueAlgebra::new(&domain)
            .borrowed()
            .pack_word_numeric(&refs)
    );
    result
}

#[test]
fn captured_low_five_zero_and_all_unknown_match_full_scalar_fold() {
    for capacity in [1, 4, 8, 64, 512] {
        for rounds in [4, 16] {
            let domain = policy(Profile::Product, capacity, rounds, 4096);
            let bytes = captured();
            let packed = equivalent(domain, &bytes);
            assert_eq!(packed.known_bits().zero(), U256::from(31));
            assert_eq!(packed.congruence(), &bytes[31].congruence);
            assert!(!packed.nonzero);
            assert!(!packed.contains(U256::ONE));
            assert!(packed.contains(U256::MAX - U256::from(31)));
            assert_eq!(packed.finite, FiniteConstantSet::top());
            let unknown = std::array::from_fn(|_| NumericValue::unknown_byte());
            assert_eq!(equivalent(domain, &unknown), NumericValue::top());
        }
    }
}

#[test]
fn mixed_constants_cubes_and_redundant_components_match_in_arbitrary_lanes() {
    for capacity in [1, 8, 512] {
        let domain = policy(Profile::Product, capacity, 16, 4096);
        for position in [0, 1, 7, 8, 15, 16, 23, 30, 31] {
            let mut bytes = std::array::from_fn(|i| NumericValue::constant(U256::from(i * 7)));
            bytes[position] = cube(0x81, 0x24, position % 2 == 0);
            bytes[(position + 17) % 32] = cube(0x12, 0x80, position % 2 != 0);
            // The positive low prefix makes both this congruence and NonZero
            // redundant, while the finite component can be present or absent.
            bytes[position].congruence = Congruence::top();
            bytes[position].nonzero = true;
            equivalent(domain, &bytes);
        }
    }
}

#[test]
fn every_word_limb_and_sign_boundary_keeps_full_finite_presentation() {
    for bit in [0, 1, 7, 8, 63, 64, 127, 128, 247, 248, 254, 255] {
        for capacity in [1, 2, 8, 512] {
            let domain = policy(Profile::Product, capacity, 4, 4096);
            for fixed in [0u8, !(1u8 << (bit % 8))] {
                let mut bytes = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
                bytes[31 - bit / 8] = cube(1 << (bit % 8), fixed, bit % 2 == 0);
                let packed = equivalent(domain, &bytes);
                assert_eq!(
                    packed.constants().map(BTreeSet::len),
                    (capacity >= 2).then_some(2)
                );
            }
        }
    }
}

#[test]
fn bounded_cube_concatenation_agrees_with_independent_concrete_bytes() {
    let domain = policy(Profile::Product, 512, 16, 4096);
    for (left, right) in [(0, 31), (1, 30), (8, 16)] {
        let mut bytes = std::array::from_fn(|i| NumericValue::constant(U256::from(i)));
        bytes[left] = cube(0x81, 0x24, true);
        bytes[right] = cube(0x12, 0x80, false);
        let packed = equivalent(domain, &bytes);
        let mut concrete = BTreeSet::new();
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let mut word: [u8; 32] = std::array::from_fn(|i| i as u8);
                word[left] = a;
                word[right] = b;
                let valid =
                    bytes[left].contains(U256::from(a)) && bytes[right].contains(U256::from(b));
                let word = U256::from_be_bytes(word);
                assert_eq!(packed.contains(word), valid);
                if valid {
                    concrete.insert(word);
                }
            }
        }
        assert_eq!(packed.constants(), Some(&concrete));
        assert_eq!(concrete.len(), 16);
    }
}

#[test]
fn constrained_counterexamples_are_not_cube_certificates() {
    let domain = policy(Profile::Product, 512, 16, 4096);
    let mut sparse = cube(3, 0, true);
    sparse.finite =
        FiniteConstantSet::try_from_values(BTreeSet::from([U256::ZERO, U256::from(3)])).unwrap();
    let mut nonzero = NumericValue::unknown_byte();
    nonzero.nonzero = true;
    let mut odd_congruence = NumericValue::unknown_byte();
    odd_congruence.congruence = Congruence::new(U256::from(3), U256::ONE).unwrap();
    let mut truncated = NumericValue::unknown_byte();
    truncated.interval = Interval::new_unsigned(U256::ZERO, U256::from(254)).unwrap();
    let mut wrong_prefix = cube(0xe0, 0, false);
    wrong_prefix.congruence = Congruence::new(U256::from(64), U256::ZERO).unwrap();
    let mut signed = NumericValue::unknown_byte();
    let sign = U256::ONE << 255usize;
    signed.interval = Interval::new_signed(sign + U256::ONE, sign + U256::from(255)).unwrap();
    let mut superset = cube(0xe0, 0, true);
    let mut values = superset.constants().unwrap().clone();
    values.insert(U256::ONE);
    superset.finite = FiniteConstantSet::try_from_values(values).unwrap();
    for rejected in [
        sparse,
        nonzero,
        odd_congruence,
        truncated,
        wrong_prefix,
        signed,
        superset,
        NumericValue::top(),
    ] {
        let mut bytes = captured();
        bytes[31] = rejected;
        assert!(
            domain
                .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
                .is_none()
        );
    }
}

fn singleton_forms(exact: U256) -> [NumericValue; 7] {
    let interval = NumericValue::unsigned_range(exact, exact).unwrap();
    let mut bits = NumericValue::top();
    bits.bits = KnownBits::exact(exact);
    let mut congruence = NumericValue::top();
    congruence.congruence = Congruence::exact(exact);
    let mut finite = NumericValue::top();
    finite.finite = FiniteConstantSet::constant(exact);
    let mut superset = bits.clone();
    superset.finite =
        FiniteConstantSet::try_from_values(BTreeSet::from([exact, exact ^ U256::ONE])).unwrap();
    let mut interval_modulo = interval.clone();
    interval_modulo.congruence = Congruence::new(U256::from(3), exact % U256::from(3)).unwrap();
    let mut bits_modulo = bits.clone();
    bits_modulo.congruence = interval_modulo.congruence.clone();
    [
        interval,
        bits,
        congruence,
        finite,
        superset,
        interval_modulo,
        bits_modulo,
    ]
}

#[test]
fn product_component_singletons_keep_the_existing_concrete_packing_admission() {
    for domain in [
        policy(Profile::Product, 512, 16, 4096),
        policy(Profile::Product, 1, 1, 1),
    ] {
        for exact in [0u8, 7, 127, 128, 255] {
            for value in singleton_forms(U256::from(exact)) {
                assert_eq!(value.singleton(), Some(U256::from(exact)));
                let bytes: [_; 32] = std::array::from_fn(|_| value.clone());
                assert_eq!(
                    equivalent(domain, &bytes),
                    NumericValue::constant(U256::from_be_bytes([exact; 32]))
                );
            }
        }
    }
}

#[test]
fn component_singletons_mix_with_certified_cubes_without_losing_constraints() {
    for capacity in [1, 8, 512] {
        let domain = policy(Profile::Product, capacity, 16, 4096);
        let mut bytes = std::array::from_fn(|_| {
            NumericValue::unsigned_range(U256::from(7), U256::from(7)).unwrap()
        });
        bytes[0] = cube(0x81, 0x24, false);
        bytes[31] = captured()[31].clone();
        equivalent(domain, &bytes);

        // This exact congruence is not a nonconstant cube certificate; it is
        // the established validated-singleton case, with the same raw input.
        let mut bytes = captured();
        bytes[31] = NumericValue::unknown_byte();
        bytes[31].congruence = Congruence::exact(U256::from(32));
        let packed = equivalent(domain, &bytes);
        assert_eq!(packed.known_bits().one() & U256::from(255), U256::from(32));
        assert_eq!(
            packed.known_bits().zero() & U256::from(255),
            U256::from(223)
        );
    }
}

#[test]
fn singleton_candidates_must_satisfy_every_own_constraint() {
    let domain = policy(Profile::Product, 512, 16, 4096);
    let seven = U256::from(7);
    let eight = U256::from(8);
    let mut wrong_modulo = NumericValue::unsigned_range(seven, seven).unwrap();
    wrong_modulo.congruence = Congruence::new(U256::from(3), U256::ZERO).unwrap();
    let mut wrong_bits = NumericValue::unsigned_range(seven, seven).unwrap();
    wrong_bits.bits = KnownBits::exact(eight);
    let mut wrong_finite = NumericValue::constant(seven);
    wrong_finite.bits = KnownBits::exact(eight);
    let mut wrong_exact = NumericValue::unsigned_range(eight, eight).unwrap();
    wrong_exact.congruence = Congruence::exact(seven);
    let mut excluded_zero = NumericValue::unsigned_range(U256::ZERO, U256::ZERO).unwrap();
    excluded_zero.nonzero = true;
    let mut missing_member = NumericValue::unsigned_range(seven, seven).unwrap();
    missing_member.finite =
        FiniteConstantSet::try_from_values(BTreeSet::from([U256::from(6), eight])).unwrap();
    let mut bits_modulo = NumericValue::top();
    bits_modulo.bits = KnownBits::exact(seven);
    bits_modulo.congruence = wrong_modulo.congruence.clone();
    for invalid in [
        wrong_modulo,
        wrong_bits,
        wrong_finite,
        wrong_exact,
        excluded_zero,
        missing_member,
        bits_modulo,
    ] {
        assert_eq!(invalid.singleton(), None);
        let bytes: [_; 32] = std::array::from_fn(|_| invalid.clone());
        assert!(
            domain
                .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
                .is_none()
        );
    }
    // Interval itself excludes reversed bounds; contradictions above are
    // representable intersections of individually valid numeric components.
    assert!(NumericValue::unsigned_range(eight, seven).is_none());
}

#[test]
fn wide_singletons_and_constants_only_without_finite_literals_still_decline() {
    let product = policy(Profile::Product, 512, 16, 4096);
    let constants = policy(Profile::ConstantsOnly, 512, 16, 4096);
    for exact in [U256::from(256), U256::MAX] {
        for value in singleton_forms(exact) {
            assert_eq!(value.singleton(), Some(exact));
            let bytes: [_; 32] = std::array::from_fn(|_| value.clone());
            for domain in [product, constants] {
                assert!(
                    domain
                        .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
                        .is_none()
                );
            }
        }
    }
    for value in singleton_forms(U256::from(7)) {
        let bytes: [_; 32] = std::array::from_fn(|_| value.clone());
        if value.finite.cardinality() == Some(1) {
            equivalent(constants, &bytes);
        } else {
            assert!(
                constants
                    .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
                    .is_none()
            );
        }
    }
}

#[test]
fn singleton_quotes_fund_discovery_without_changing_the_captured_cube_fees() {
    use crate::domain::WordElement;
    let domain = policy(Profile::Product, 512, 16, 4096);
    let unknown: [_; 32] = std::array::from_fn(|_| NumericValue::unknown_byte());
    assert_eq!(
        domain.pack_word_work(&std::array::from_fn(|i| &unknown[i])),
        Some(10_240)
    );
    let captured = captured();
    assert_eq!(
        domain.pack_word_work(&std::array::from_fn(|i| &captured[i])),
        Some(10_464)
    );
    let canonical: [_; 32] = std::array::from_fn(|_| NumericValue::constant(U256::from(7)));
    let canonical_fee = domain
        .pack_word_work(&std::array::from_fn(|i| &canonical[i]))
        .unwrap();
    assert_eq!(canonical_fee, 10_816);
    for (value, query) in singleton_forms(U256::from(7))
        .into_iter()
        .zip([true, false, true, true, true, true, true])
    {
        let bytes: [_; 32] = std::array::from_fn(|_| value.clone());
        let fee = domain
            .pack_word_work(&std::array::from_fn(|i| &bytes[i]))
            .unwrap();
        // Hold input cardinality/output fixed to isolate the paid query. The
        // superset case also adds its independent existing input-member fee.
        let members = 32 * (value.finite.work_size() - 1) * 32;
        let discovery = if query {
            32 * value.singleton_work()
        } else {
            0
        };
        assert_eq!(fee, canonical_fee + members + discovery);
    }
}

#[test]
fn output_quote_overflow_still_refuses_before_finite_construction() {
    let domain = policy(
        Profile::Product,
        1usize << (usize::BITS - 1),
        16,
        usize::MAX,
    );
    let bytes: [_; 32] = std::array::from_fn(|i| {
        let remaining = (usize::BITS as usize - 1).saturating_sub(8 * i).min(8);
        cube(((1u16 << remaining) - 1) as u8, 0, false)
    });
    assert_eq!(
        domain.pack_word_work(&std::array::from_fn(|i| &bytes[i])),
        Some(usize::MAX)
    );
}

#[test]
fn small_cooperation_limits_and_constants_only_keep_conservative_admission() {
    for domain in [
        policy(Profile::Product, 512, 1, 4096),
        policy(Profile::Product, 512, 16, 512),
        policy(Profile::ConstantsOnly, 512, 16, 4096),
    ] {
        let bytes = captured();
        assert!(
            domain
                .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
                .is_none()
        );
        let bytes = std::array::from_fn(|i| NumericValue::constant(U256::from(i)));
        equivalent(domain, &bytes);
    }
    let domain = policy(Profile::ConstantsOnly, 8, 4, 256);
    let mut bytes: [_; 32] = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
    // A singleton established only by bits is not a ConstantsOnly literal.
    bytes[31].finite = FiniteConstantSet::top();
    assert!(
        domain
            .pack_word_numeric(&std::array::from_fn(|i| &bytes[i]))
            .is_none()
    );
}
