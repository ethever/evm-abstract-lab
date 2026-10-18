// Full-value reference probes share the existing ordered-loop oracle.

use super::super::super::{Context, full_image, scalar_in};
use super::super::{Facts, Reader, constant, expression, original_union};
use super::{counted_union, original_in_context, product};
use crate::domain::{
    AbstractValue, Domain, DomainSpec, Profile,
    facts::{BitConstraints, UnaryPredicate, WordBounds},
    identity::Symbol,
    provenance::{Origin, OriginSet},
    relational::RelationLimits,
    symbolic::{ByteStateId, ByteView, ExprId, ExprKind, ExprLimits, SourceId},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};

fn cube(zero: U256, one: U256, nonzero: bool) -> AbstractValue {
    let mut facts = vec![UnaryPredicate::KnownBits(
        BitConstraints::new(zero, one).unwrap(),
    )];
    if nonzero {
        facts.push(UnaryPredicate::NonZero);
    }
    product(8, 16, 4096).from_facts(&facts).unwrap()
}

fn word_read() -> ExprId {
    ExprId::word_read(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32))),
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(32)),
        ),
        ExprId::constant(U256::ZERO),
        ExprLimits::default(),
    )
    .unwrap()
}

fn check(value: &AbstractValue, key: &ExprId, domain: Domain, calls: (usize, usize)) {
    let facts = Facts::from([(key.clone(), value.clone())]);
    let frozen = facts.clone();
    let expected = original_union(key, &facts, domain);
    let mut reader = Reader::new(&facts, domain);
    let actual = counted_union(&mut reader, key, &expected, calls);
    // Spell out independent layers so a future equality implementation cannot
    // silently weaken this oracle's complete representation requirement.
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(
        actual.symbolic_limit_reached(),
        value.symbolic_limit_reached()
    );
    assert!(!actual.provenance().is_code_address());
    assert!(!actual.same_identity(value));
    assert!(actual.identity().input().is_none());
    assert!(actual.expression().is_none());
    assert_eq!(
        reader.cache_stats(),
        (1, key.work_size() + actual.work_size())
    );
    counted_union(&mut reader, key, &expected, (0, 0));
    assert_eq!(facts, frozen);
}

#[test]
fn observed_sign_clear_word_read_preserves_full_av_at_normal_and_fact_edges() {
    let sign = U256::from(1) << 255;
    let observed = cube(sign, U256::ZERO, false);
    assert!(observed.finite_constants().is_top());
    assert!(observed.congruence().is_top());
    assert_eq!(observed.known_bits().zero(), sign);
    assert_eq!(observed.known_bits().one(), U256::ZERO);
    assert_eq!(observed.interval().unsigned_bounds(), (U256::ZERO, !sign));
    assert_eq!(observed.interval().signed_bounds(), (sign, U256::MAX));
    assert!(observed.may_be_zero());
    let key = word_read();
    assert!(matches!(key.kind(), ExprKind::WordRead { .. }));
    let observed = observed.with_expression(key.clone());
    let metadata = product(8, 16, 4096)
        .from_facts(&[
            UnaryPredicate::PossibleOrigins(
                OriginSet::from_sources([Origin::Calldata, Origin::Storage]).unwrap(),
            ),
            UnaryPredicate::IsCodeAddress,
        ])
        .unwrap()
        .with_numeric(observed.numeric().clone())
        .with_symbol(Symbol::CalldataWord(U256::from(32)), Some(811_200))
        .with_identity(811_201, 9)
        .with_expression(key.clone())
        .with_symbolic_limit();
    assert!(metadata.provenance().is_code_address());
    assert!(metadata.identity().input().is_some());
    assert!(metadata.same_identity(&metadata));
    for capacity in [8, 512] {
        // 260/261 and 263/264 straddle 256 candidates + four scalar
        // slots + respectively Top origins (one) or four transferred sources.
        for (rounds, fact_limit) in [
            (16, 4096),
            (1, 1),
            (1, 256),
            (1, 260),
            (1, 261),
            (1, 263),
            (1, 264),
        ] {
            let domain = product(capacity, rounds, fact_limit);
            for value in [&observed, &metadata] {
                assert!(full_image::candidate(value.numeric(), domain));
                assert!(full_image::proves(value.numeric()));
                check(value, &key, domain, (1, 0));
            }
        }
    }
    let domain = product(512, 16, 4096);
    let facts = Facts::from([(key.clone(), metadata)]);
    let expected = original_union(&key, &facts, domain);
    assert_eq!(expected.constants().unwrap().len(), 256);
    let sources = expected.provenance().origins().sources().unwrap();
    assert_eq!(
        sources,
        &BTreeSet::from([
            Origin::Calldata,
            Origin::Storage,
            Origin::Constant,
            Origin::Arithmetic,
        ])
    );
}

#[test]
fn general_free_lanes_known_ones_and_nonzero_keep_the_full_union() {
    let sign = U256::from(1) << 255;
    let zero_witness = cube(!U256::from(255), U256::ZERO, true);
    assert!(!zero_witness.may_be_zero());
    assert_eq!(zero_witness.known_bits().one(), U256::ZERO);
    let values = [
        AbstractValue::unknown_address(),
        cube(U256::ZERO, sign, false),
        cube((U256::from(1) << 200) | (U256::from(1) << 100), sign, true),
        zero_witness,
        product(8, 16, 4096)
            .from_facts(&[UnaryPredicate::NonZero])
            .unwrap(),
    ];
    for capacity in [8, 512] {
        let domain = product(capacity, 16, 4096);
        for value in &values {
            assert!(value.finite_constants().is_top());
            assert!(value.congruence().is_top());
            assert!(!value.numeric().is_top());
            assert!(full_image::candidate(value.numeric(), domain));
            assert!(full_image::proves(value.numeric()));
            let key = expression();
            check(value, &key, domain, (1, 0));
            let facts = Facts::from([(key.clone(), value.clone())]);
            let actual = Reader::new(&facts, domain).word_byte_union(&key);
            assert!(actual.contains(U256::ZERO));
            assert!(actual.contains(U256::from(255)));
        }
    }
    // The first byte of an address is zero, despite its full all-position image.
    assert_eq!(
        product(8, 16, 4096)
            .apply(opcode::BYTE, &[constant(0), values[0].clone()])
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn noncertificates_keep_all_32_byte_transfers_and_31_joins() {
    let constructor = product(8, 16, 4096);
    let repeated = constructor
        .from_facts(&[
            UnaryPredicate::KnownBits(
                BitConstraints::new(U256::from_be_bytes([0xdf; 32]), U256::ZERO).unwrap(),
            ),
            UnaryPredicate::congruent(U256::from(32), U256::ZERO).unwrap(),
        ])
        .unwrap();
    assert!(repeated.finite_constants().is_top());
    assert_eq!(
        repeated.known_bits().zero(),
        U256::from_be_bytes([0xdf; 32])
    );
    assert_eq!(
        repeated.congruence().modulus_residue(),
        Some((U256::from(32), U256::ZERO))
    );
    let sign = U256::from(1) << 255;
    let signed = product(1, 16, 4096)
        .from_facts(&[UnaryPredicate::SignedBounds(
            WordBounds::new(sign - U256::from(1), sign + U256::from(1)).unwrap(),
        )])
        .unwrap();
    assert!(signed.finite_constants().is_top());
    assert!(signed.congruence().is_top());
    assert_eq!(signed.interval().unsigned_bounds(), (U256::ZERO, U256::MAX));
    let exact_congruence = product(1, 1, 1)
        .from_facts(&[UnaryPredicate::congruent(U256::MAX, U256::from(1)).unwrap()])
        .unwrap();
    assert!(exact_congruence.finite_constants().is_top());
    assert_eq!(
        exact_congruence.congruence().singleton(),
        Some(U256::from(1))
    );
    let values = [
        repeated,
        signed,
        AbstractValue::unsigned_range(U256::ZERO, U256::from(127)).unwrap(),
        constructor.join(&constant(0), &constant(32)),
        cube(U256::from_be_bytes([0x80; 32]), U256::ZERO, false),
        constructor
            .from_facts(&[UnaryPredicate::congruent(U256::from(3), U256::ONE).unwrap()])
            .unwrap(),
        exact_congruence,
        AbstractValue::constant(U256::from_be_bytes([0xaa; 32])),
    ];
    for capacity in [8, 512] {
        let domain = product(capacity, 16, 4096);
        for (index, value) in values.iter().enumerate() {
            assert!(
                !full_image::candidate(value.numeric(), domain)
                    || !full_image::proves(value.numeric())
            );
            let key = if index == 0 {
                word_read()
            } else {
                expression()
            };
            check(value, &key, domain, (32, 31));
        }
        let key = word_read();
        let facts = Facts::from([(key.clone(), values[0].clone())]);
        let expected = original_union(&key, &facts, domain);
        assert_eq!(
            expected.constants(),
            Some(&BTreeSet::from([U256::ZERO, U256::from(32)]))
        );
    }
}

#[test]
fn certificate_does_not_change_constants_only_or_relations_on() {
    let key = expression();
    let input = cube(U256::from(1) << 255, U256::ZERO, false)
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(811_202))
        .with_symbolic_limit();
    let facts = Facts::from([(key.clone(), input)]);
    for (profile, enabled) in [(Profile::ConstantsOnly, false), (Profile::Product, true)] {
        let domain = Domain::from_spec(
            DomainSpec::new(
                profile,
                NonZeroUsize::new(8).unwrap(),
                NonZeroUsize::new(16).unwrap(),
                NonZeroUsize::new(4096).unwrap(),
            )
            .with_relations(RelationLimits {
                enabled,
                ..Default::default()
            }),
        );
        let mut reader = Reader::with_context(Context::new(&facts, None), domain);
        reader.domain = domain;
        let scalar = scalar_in(&key, &reader.context, domain);
        assert!(!full_image::candidate(scalar.numeric(), domain));
        let expected = original_in_context(&key, &reader.context, domain);
        counted_union(&mut reader, &key, &expected, (32, 31));
        counted_union(&mut reader, &key, &expected, (0, 0));
    }
}
