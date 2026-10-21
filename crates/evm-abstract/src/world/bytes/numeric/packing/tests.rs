use crate::domain::{AlgebraRef, EvmNumericAlgebra, EvmValue, WordElement};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};
use {
    crate::domain::DomainSpec, crate::domain::NumericValue, crate::domain::facts::FiniteSet,
    crate::domain::facts::UnaryPredicate, crate::domain::facts::WordBounds,
    crate::domain::identity::Symbol, crate::domain::provenance::Origin,
    crate::domain::relational::RelationLimits, crate::domain::symbolic::ByteStateId,
    crate::domain::symbolic::ByteView, crate::domain::symbolic::ExprId,
    crate::domain::symbolic::ExprKind, crate::domain::symbolic::ExprLimits,
    crate::domain::symbolic::SourceId, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestValue,
};

// Existing direct probes exercise the pure library contract without a ledger.
fn pack<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    bytes: &[EvmValue<N>],
    domain: AlgebraRef<'_, D>,
) -> Option<EvmValue<N>> {
    let facts = super::super::Facts::new();
    let context = super::super::Context::new(&facts, None, domain);
    super::pack(bytes, domain, &context)
}

mod cubes;
mod payer;

#[test]
fn declining_custom_packing_hook_borrows_all_heavy_elements() {
    use crate::world::bytes::carrier_tests::{HEAVY_CLONES, HeavyWords, heavy_value};
    let _scope = HEAVY_CLONES.scope();
    let algebra = crate::domain::ValueAlgebra::new(HeavyWords::default());
    let bytes: [_; 32] = std::array::from_fn(|_| heavy_value(64 * 1024));
    HEAVY_CLONES.replace(0);
    assert!(pack(&bytes, algebra.borrowed().without_relations()).is_none());
    assert_eq!(algebra.numeric_algebra().packs.get(), 1);
    assert_eq!(
        HEAVY_CLONES.replace(0),
        0,
        "declining hook deep-cloned its inputs"
    );
}

const SOURCES: [Origin; 11] = [
    Origin::Calldata,
    Origin::Memory,
    Origin::Storage,
    Origin::TransientStorage,
    Origin::Environment,
    Origin::Address,
    Origin::CodeAddress,
    Origin::CallValue,
    Origin::Returndata,
    Origin::Balance,
    Origin::Nonce,
];

fn policies(enabled: bool) -> Vec<TestAlgebra> {
    [(1, 1, 1), (8, 4, 256), (256, 1, 256)]
        .map(|(capacity, rounds, facts)| {
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
                DomainSpec::new(
                    NonZeroUsize::new(capacity).unwrap(),
                    NonZeroUsize::new(rounds).unwrap(),
                    NonZeroUsize::new(facts).unwrap(),
                )
                .with_relations(RelationLimits {
                    enabled,
                    ..Default::default()
                }),
            ))
        })
        .into_iter()
        .collect()
}

// The original Reader.word fold, including its zero shift and balanced OR tree.
// Keep all 63 transfers as the independent full-AbstractValue oracle.
fn original<D: EvmNumericAlgebra>(
    bytes: &[EvmValue<D::Element>],
    domain: crate::domain::ValueAlgebra<D>,
) -> EvmValue<D::Element> {
    assert_eq!(bytes.len(), 32);
    let mut parts = bytes
        .iter()
        .enumerate()
        .map(|(j, b)| {
            domain.apply(
                opcode::SHL,
                &[
                    EvmValue::<D::Element>::constant(U256::from((31 - j) * 8)),
                    b.clone(),
                ],
            )
        })
        .collect::<Vec<_>>();
    while parts.len() > 1 {
        parts = parts
            .chunks(2)
            .map(|p| {
                if p.len() == 2 {
                    domain.apply(opcode::OR, p)
                } else {
                    p[0].clone()
                }
            })
            .collect();
    }
    parts.pop().unwrap()
}

fn operand(numeric: NumericValue, index: usize) -> TestValue {
    let mut value = TestValue::top();
    value.set_origin(SOURCES[index % SOURCES.len()]);
    value
        .with_symbol(Symbol::CalldataWord(U256::from(index)), Some(73))
        .with_identity(91, u32::try_from(index).unwrap())
        .with_numeric(numeric)
}

fn assert_fresh(value: &TestValue, bytes: &[TestValue]) {
    assert!(bytes[0].same_identity(&bytes[0].clone()));
    assert!(value.identity().input().is_none());
    // Persistent equality deliberately ignores runtime identity; test it too.
    assert!(!value.same_identity(value));
    assert!(bytes.iter().all(|b| !value.same_identity(b)));
    assert!(value.expression().is_none());
    assert!(!value.provenance().is_code_address());
}

fn assert_sources(value: &TestValue) {
    let expected = SOURCES
        .into_iter()
        .chain([Origin::Constant, Origin::Arithmetic])
        .collect::<BTreeSet<_>>();
    assert_eq!(value.provenance().origins().sources(), Some(&expected));
}

fn unknown_pack(bytes: &[TestValue], domain: TestAlgebra) -> TestValue {
    let packed = pack(bytes, domain.borrowed());
    let spec = domain.spec();
    assert_eq!(
        packed.is_some(),
        spec.reduction_rounds() >= 4 && spec.fact_limit() >= spec.capacity() + 8,
        "unsupported cooperation controls must decline"
    );
    packed.unwrap_or_else(|| original(bytes, domain))
}

#[test]
fn concrete_big_endian_words_match_full_original_across_limits_and_metadata() {
    let mut first = [0_u8; 32];
    first[0] = 255;
    let mut last = [0_u8; 32];
    last[31] = 255;
    let ramp = std::array::from_fn(|i| u8::try_from(i).unwrap());
    let cases = [
        ([0_u8; 32], U256::ZERO),
        ([255_u8; 32], U256::MAX),
        (first, U256::from(255) << 248usize),
        (last, U256::from(255)),
        (
            ramp,
            U256::from_str_radix(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                16,
            )
            .unwrap(),
        ),
    ];
    for domain in policies(false).into_iter().chain(policies(true)) {
        for (raw, expected) in cases {
            let mut bytes = raw
                .into_iter()
                .enumerate()
                .map(|(i, b)| operand(NumericValue::constant(U256::from(b)), i))
                .collect::<Vec<_>>();
            bytes[31] = bytes[31].clone().with_symbolic_limit();
            let packed = pack(&bytes, domain.borrowed()).unwrap();
            assert_eq!(packed, original(&bytes, domain));
            assert_eq!(packed.numeric(), &NumericValue::constant(expected));
            assert_sources(&packed);
            assert_fresh(&packed, &bytes);
            assert!(!packed.symbolic_limit_reached());

            // Top source metadata absorbs the complete known source union.
            bytes[0] = TestValue::from_numeric(bytes[0].numeric().clone());
            assert_eq!(
                pack(&bytes, domain.borrowed()).unwrap(),
                original(&bytes, domain)
            );
            assert!(
                pack(&bytes, domain.borrowed())
                    .unwrap()
                    .provenance()
                    .is_top()
            );
        }
    }
}

#[test]
fn plain_unknowns_match_full_original_and_keep_caller_word_dependency() {
    for domain in policies(false) {
        let plain = vec![TestValue::unknown_byte(); 32];
        assert_eq!(unknown_pack(&plain, domain), original(&plain, domain));
        assert_eq!(unknown_pack(&plain, domain), TestValue::top());

        let mut bytes = (0..32)
            .map(|i| operand(NumericValue::unknown_byte(), i))
            .collect::<Vec<_>>();
        // A role is a must fact that arithmetic clears, independently of origins.
        bytes[0] = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
            .address_projection(&bytes[0])
            .with_numeric(NumericValue::unknown_byte());
        assert!(bytes[0].provenance().is_code_address());
        bytes[1] = bytes[2].clone();
        assert!(bytes[1].same_identity(&bytes[2]));
        let packed = unknown_pack(&bytes, domain);
        assert_eq!(packed, original(&bytes, domain));
        assert_eq!(packed.numeric(), &NumericValue::top());
        assert_sources(&packed);
        assert_fresh(&packed, &bytes[1..]);
        assert!(!packed.symbolic_limit_reached());

        bytes[31] = bytes[31].clone().with_symbolic_limit();
        let packed = unknown_pack(&bytes, domain);
        assert_eq!(packed, original(&bytes, domain));
        assert!(packed.symbolic_limit_reached());

        let root = ByteStateId::input(
            SourceId::input(73, Symbol::CalldataLength),
            ExprId::constant(U256::from(32)),
        );
        let expression = ExprId::word_read(
            ByteView::new(
                root.clone(),
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(32)),
            ),
            ExprId::constant(U256::ZERO),
            ExprLimits::default(),
        )
        .unwrap();
        let attached = super::super::attach(packed, expression.clone());
        assert_eq!(attached.expression(), Some(&expression));
        assert!(matches!(expression.kind(), ExprKind::WordRead { view, .. } if view.state == root));
        assert_eq!(attached.numeric(), &NumericValue::top());
        assert_sources(&attached);
        assert!(attached.symbolic_limit_reached());

        // A Top source encountered early must not mask a later limit marker.
        bytes[0] = TestValue::unknown_byte();
        let packed = unknown_pack(&bytes, domain);
        assert_eq!(packed, original(&bytes, domain));
        assert!(packed.provenance().is_top());
        assert!(packed.symbolic_limit_reached());
    }
}

#[test]
fn mixed_informative_malformed_and_wrong_length_inputs_fall_back() {
    let algebra = TestAlgebra::new(crate::domain::ProductAlgebra::default());
    let domain = super::super::numeric_domain(algebra.borrowed());
    let finite = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .from_facts(&[UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ZERO, U256::from(255)])).unwrap(),
        )])
        .unwrap();
    let nonzero = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::from(255)).unwrap()),
            UnaryPredicate::NonZero,
        ])
        .unwrap();
    let odd = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::from(255)).unwrap()),
            UnaryPredicate::congruent(U256::from(2), U256::from(1)).unwrap(),
        ])
        .unwrap();
    for special in [
        TestValue::constant(U256::ZERO),
        TestValue::constant(U256::from(255)),
    ] {
        let mut bytes = vec![TestValue::unknown_byte(); 32];
        bytes[17] = special;
        assert_eq!(pack(&bytes, domain).unwrap(), original(&bytes, domain));
    }
    for special in [
        TestValue::constant(U256::from(256)),
        TestValue::constant(U256::MAX),
        TestValue::top(),
        TestValue::unsigned_range(U256::ZERO, U256::from(255)).unwrap(),
        TestValue::unsigned_range(U256::ZERO, U256::from(127)).unwrap(),
        finite,
        odd,
        nonzero,
    ] {
        let mut bytes = vec![TestValue::unknown_byte(); 32];
        bytes[17] = special;
        assert!(pack(&bytes, domain).is_none());
    }
    for length in [0, 1, 31, 33, 64] {
        assert!(pack(&vec![TestValue::unknown_byte(); length], domain).is_none());
        assert!(pack(&vec![TestValue::constant(U256::ZERO); length], domain).is_none());
    }
    for domain in policies(false) {
        for wider in [U256::from(256), U256::MAX] {
            let mut bytes = vec![TestValue::constant(U256::ZERO); 32];
            bytes[17] = TestValue::constant(wider);
            assert!(pack(&bytes, domain).is_none());
        }
    }
    for domain in policies(true) {
        assert!(pack(&vec![TestValue::unknown_byte(); 32], domain).is_none());
    }

    // The product can use an exact interval without finite candidates.
    let interval_singleton = TestValue::unsigned_range(U256::from(7), U256::from(7)).unwrap();
    let bytes = vec![interval_singleton; 32];
    assert!(bytes[0].constants().is_none());
    assert_eq!(pack(&bytes, domain).unwrap(), original(&bytes, domain));
    // The standalone finite carrier has no product packing certificate. Its
    // unknown-byte payload is Top at this capacity; the full transfer oracle
    // must stay conservative when the optional packing hook declines.
    let constants_algebra = crate::domain::ValueAlgebra::new(
        crate::domain::FiniteSetAlgebra::from_spec(crate::domain::DomainSpec::new(
            NonZeroUsize::new(8).unwrap(),
            std::num::NonZeroUsize::new(8).unwrap(),
            std::num::NonZeroUsize::new(256).unwrap(),
        )),
    );
    let constants = super::super::numeric_domain(constants_algebra.borrowed());
    let finite_bytes = vec![EvmValue::<crate::domain::FiniteConstantSet>::unknown_byte(); 32];
    assert!(pack(&finite_bytes, constants).is_none());
    assert_eq!(
        original(&finite_bytes, constants).numeric(),
        &crate::domain::FiniteConstantSet::top()
    );
}
