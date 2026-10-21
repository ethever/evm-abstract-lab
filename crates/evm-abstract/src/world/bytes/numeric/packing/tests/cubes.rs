//! Full world values compared with the actual balanced scalar fold.

use super::*;
use crate::domain::{
    WordObservation, facts::BitConstraints, known_bits::KnownBits, symbolic::StateLimits,
};
use crate::world::bytes::numeric::{Facts, Reader};

fn policy(capacity: usize, rounds: usize, facts: usize) -> TestAlgebra {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(rounds).unwrap(),
            NonZeroUsize::new(facts).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: false,
            ..Default::default()
        }),
    ))
}

fn cube(free: u8, one: u8, finite: bool) -> NumericValue {
    assert_eq!(free & one, 0);
    let zero = !U256::from(free | one);
    if finite {
        policy(512, 16, 4096)
            .from_facts(&[
                UnaryPredicate::KnownBits(BitConstraints::new(zero, U256::from(one)).unwrap()),
                UnaryPredicate::MemberOf(
                    FiniteSet::new(
                        (0..=255u64)
                            .filter(|v| (*v as u8) & !free == one)
                            .map(U256::from)
                            .collect(),
                    )
                    .unwrap(),
                ),
            ])
            .unwrap()
            .numeric()
            .clone()
    } else {
        NumericValue::from_observation(&WordObservation::bits(
            KnownBits::new(zero, U256::from(one)).unwrap(),
        ))
    }
}

fn complete(actual: &TestValue, expected: &TestValue, bytes: &[TestValue]) {
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(
        actual.symbolic_limit_reached(),
        expected.symbolic_limit_reached()
    );
    assert_fresh(actual, &bytes[1..]);
    assert!(!actual.same_identity(&bytes[0]));
}

fn equivalent(bytes: &[TestValue], domain: TestAlgebra) -> TestValue {
    let packed = pack(bytes, domain.borrowed()).expect("exact byte cubes");
    complete(&packed, &original(bytes, domain), bytes);
    packed
}

#[test]
fn captured_and_unknown_words_keep_full_balanced_fold_at_injected_default_controls() {
    let domain = policy(512, 16, 4096);
    let mut bytes: Vec<_> = (0..32)
        .map(|i| operand(NumericValue::unknown_byte(), i).with_symbolic_limit())
        .collect();
    let unknown = equivalent(&bytes, domain);
    assert_eq!(unknown.numeric(), &NumericValue::top());
    assert!(unknown.symbolic_limit_reached());
    assert_sources(&unknown);

    bytes[31] = operand(cube(0xe0, 0, true), 31).with_symbolic_limit();
    assert_eq!(bytes[31].constants().unwrap().len(), 8);
    let packed = equivalent(&bytes, domain);
    assert_eq!(packed.known_bits().zero(), U256::from(31));
    assert_eq!(packed.known_bits().one(), U256::ZERO);
    assert_eq!(
        packed.congruence().modulus_residue(),
        Some((U256::from(32), U256::ZERO))
    );
    assert!(packed.constants().is_none());
    assert!(packed.may_be_zero());
    assert!(packed.symbolic_limit_reached());
    assert_sources(&packed);

    // An early unknown source absorbs origins but must not hide a later limit.
    bytes[0] = TestValue::unknown_byte();
    let packed = equivalent(&bytes, domain);
    assert!(packed.provenance().is_top());
    assert!(packed.symbolic_limit_reached());
}

#[test]
fn arbitrary_lanes_sign_and_finite_capacity_keep_balanced_fold_presentation() {
    for capacity in [1, 8, 16, 512] {
        let domain = policy(capacity, 16, 4096);
        for position in [0, 1, 7, 8, 15, 16, 23, 30, 31] {
            let mut bytes: Vec<_> = (0..32)
                .map(|i| operand(NumericValue::constant(U256::from(i * 7)), i))
                .collect();
            bytes[position] =
                operand(cube(0x81, 0x24, position % 2 == 0), position).with_symbolic_limit();
            bytes[(position + 17) % 32] = operand(cube(0x12, 0x80, position % 2 != 0), 17);
            let packed = equivalent(&bytes, domain);
            assert_eq!(
                packed.constants().map(BTreeSet::len),
                (capacity >= 16).then_some(16)
            );
            assert!(!packed.may_be_zero());
            assert!(packed.symbolic_limit_reached());
        }
    }
    let domain = policy(512, 16, 4096);
    for bit in [0, 1, 7, 8, 63, 64, 127, 128, 247, 248, 254, 255] {
        let mut bytes: Vec<_> = (0..32)
            .map(|i| operand(NumericValue::constant(U256::ZERO), i))
            .collect();
        bytes[31 - bit / 8] = operand(cube(1 << (bit % 8), 0, bit % 2 == 0), 0);
        let packed = equivalent(&bytes, domain);
        assert_eq!(
            packed.constants(),
            Some(&BTreeSet::from([U256::ZERO, U256::ONE << bit]))
        );
    }
}

#[test]
fn sparse_range_nonzero_and_signed_constraints_decline_then_keep_original_fold() {
    let domain = policy(512, 16, 4096);
    let sign = U256::ONE << 255usize;
    let bounds =
        UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::from(255)).unwrap());
    let candidates = [
        vec![UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ZERO, U256::from(32), U256::from(64)])).unwrap(),
        )],
        vec![UnaryPredicate::UnsignedBounds(
            WordBounds::new(U256::ZERO, U256::from(254)).unwrap(),
        )],
        vec![bounds.clone(), UnaryPredicate::NonZero],
        vec![
            bounds.clone(),
            UnaryPredicate::congruent(U256::from(3), U256::ONE).unwrap(),
        ],
        vec![
            bounds,
            UnaryPredicate::SignedBounds(WordBounds::new(sign, sign + U256::from(254)).unwrap()),
        ],
    ];
    for predicates in candidates {
        let special = domain.from_facts(&predicates).unwrap();
        let mut bytes: Vec<_> = (0..32)
            .map(|i| operand(NumericValue::unknown_byte(), i))
            .collect();
        bytes[17] = special.with_symbolic_limit();
        assert!(pack(&bytes, domain.borrowed()).is_none());
        let expected = original(&bytes, domain);
        assert!(expected.symbolic_limit_reached());
        assert!(expected.expression().is_none());

        let key = ExprId::fresh().unwrap();
        let facts = Facts::from([(key.clone(), bytes[17].clone())]);
        let state =
            ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32)))
                .write_byte(
                    ExprId::constant(U256::from(17)),
                    key,
                    StateLimits::default(),
                )
                .unwrap();
        let algebra = domain.borrowed();
        let mut reader = Reader::new(&facts, algebra);
        let projected: Vec<_> = (0..32)
            .map(|i| reader.read(&state, &ExprId::constant(U256::from(i))))
            .collect();
        assert!(pack(&projected, algebra).is_none());
        let actual = reader.word(&state, &ExprId::constant(U256::ZERO));
        assert_eq!(actual, original(&projected, domain));
        assert!(actual.symbolic_limit_reached());
    }
}

#[test]
fn actual_reader_keeps_captured_and_unknown_state_values_and_metadata() {
    let domain = policy(512, 16, 4096);
    let root = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32)));
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(
        key.clone(),
        operand(cube(0xe0, 0, true), 31).with_symbolic_limit(),
    )]);
    let captured = root
        .write_byte(
            ExprId::constant(U256::from(31)),
            key,
            StateLimits::default(),
        )
        .unwrap();
    for state in [&root, &captured] {
        let algebra = domain.borrowed();
        let mut reader = Reader::new(&facts, algebra);
        let bytes: Vec<_> = (0..32)
            .map(|i| reader.read(state, &ExprId::constant(U256::from(i))))
            .collect();
        let expected = original(&bytes, domain);
        assert_eq!(pack(&bytes, algebra), Some(expected.clone()));
        let actual = reader.word(state, &ExprId::constant(U256::ZERO));
        assert_eq!(actual, expected);
        assert_eq!(actual.identity(), expected.identity());
        assert_eq!(actual.expression(), expected.expression());
        assert_eq!(
            actual.symbolic_limit_reached(),
            expected.symbolic_limit_reached()
        );
    }
}

#[test]
fn insufficient_cooperation_declines_without_relaxing_the_injected_controls() {
    let bytes: Vec<_> = (0..32)
        .map(|i| {
            operand(
                if i == 31 {
                    cube(0xe0, 0, true)
                } else {
                    NumericValue::unknown_byte()
                },
                i,
            )
        })
        .collect();
    for domain in [policy(512, 1, 4096), policy(512, 16, 512)] {
        assert!(pack(&bytes, domain.borrowed()).is_none());
    }
}
