use super::{
    AbstractValue, Congruence, Domain, FiniteConstantSet, Interval, KnownBits, NumericValue,
    Profile, Provenance, projection_scalar,
};
use crate::domain::{
    DomainSpec,
    identity::Symbol,
    provenance::{Origin, OriginSet},
    relational::RelationLimits,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};

fn product(capacity: usize) -> Domain {
    Domain::from_spec(
        DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: false,
            ..RelationLimits::default()
        }),
    )
}

fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

#[test]
fn every_low8_free_mask_and_byte_index_covers_independent_concrete_observations() {
    let domain = product(8);
    // Exhaust all disjoint free/one masks (each low bit is free, zero or one).
    // Move each cube through all 32 lanes, with both outside fills.
    // Concrete semantics use the byte array, independently of domain transfers.
    for free in 0..=255_u8 {
        for one in 0..=255_u8 {
            if one & free != 0 {
                continue;
            }
            let zero = !(free | one);
            for index in 0..32 {
                let shift = (31 - index) * 8;
                let lane = U256::from(255) << shift;
                let mut numeric = NumericValue::top();
                numeric.bits =
                    KnownBits::new(U256::from(zero) << shift, U256::from(one) << shift).unwrap();
                let scalar = AbstractValue::from_numeric(numeric);
                let byte_args = [constant(index), scalar.clone()];
                let low_args = [scalar.clone(), constant(255)];
                assert!(projection_scalar(domain, opcode::BYTE, &byte_args).is_some());
                assert!(projection_scalar(domain, opcode::AND, &low_args).is_some());
                let byte = domain.observe_byte(opcode::BYTE, &byte_args);
                let low = domain.observe_byte(opcode::AND, &low_args);
                let mut assignment = free;
                loop {
                    for outside in [U256::ZERO, !lane] {
                        let concrete = (U256::from(one | assignment) << shift) | outside;
                        assert!(scalar.contains(concrete));
                        let bytes = concrete.to_be_bytes::<32>();
                        assert!(byte.contains(U256::from(bytes[index])));
                        assert!(low.contains(U256::from(bytes[31])));
                    }
                    if assignment == 0 {
                        break;
                    }
                    assignment = (assignment - 1) & free;
                }
            }
        }
    }
    for index in (32_u64..=255).map(U256::from).chain([U256::MAX]) {
        let args = [AbstractValue::constant(index), AbstractValue::top()];
        assert_eq!(
            domain.observe_byte(opcode::BYTE, &args),
            domain.apply(opcode::BYTE, &args)
        );
        assert_eq!(
            domain.observe_byte(opcode::BYTE, &args).singleton(),
            Some(U256::ZERO)
        );
    }
}

#[test]
fn mixed_open_constraints_keep_sound_component_facts_without_reduction() {
    let domain = product(8);
    let scalar = AbstractValue::from_numeric(NumericValue {
        finite: FiniteConstantSet::top(),
        bits: KnownBits::new(U256::from(0x50), U256::from(0xa0)).unwrap(),
        interval: Interval::new_unsigned(U256::from(0x90), U256::from(0x2af)).unwrap(),
        congruence: Congruence::new(U256::from(3), U256::ONE).unwrap(),
        nonzero: true,
    });
    let low = domain.observe_byte(opcode::AND, &[scalar.clone(), constant(255)]);
    assert!(low.finite_constants().is_top());
    assert_eq!(low.known_bits().one(), U256::from(0xa0));
    assert_eq!(
        low.interval().unsigned_bounds(),
        (U256::ZERO, U256::from(255))
    );
    assert_eq!(low.congruence(), &Congruence::top());
    assert!(!low.numeric.nonzero);
    let bytes =
        [30, 31].map(|index| domain.observe_byte(opcode::BYTE, &[constant(index), scalar.clone()]));
    let mut included = 0;
    for concrete in (0_u64..=0x2af)
        .map(U256::from)
        .filter(|v| scalar.contains(*v))
    {
        included += 1;
        let concrete = concrete.to_be_bytes::<32>();
        assert!(low.contains(U256::from(concrete[31])));
        for (result, index) in bytes.iter().zip([30, 31]) {
            assert!(result.contains(U256::from(concrete[index])));
        }
    }
    assert!(included > 1);
}

#[test]
fn exact_and_complete_finite_scalars_keep_the_original_complete_value() {
    for capacity in [1, 8, 256] {
        let domain = product(capacity);
        let scalar = Domain::finite(
            FiniteConstantSet::try_from_values(
                (0..capacity).map(|n| U256::from(n * 0x10101)).collect(),
            )
            .unwrap(),
        )
        .with_origin(Origin::Storage)
        .with_symbol(Symbol::Caller, Some(51))
        .with_identity(51, 3)
        .with_symbolic_limit();
        for index in 0..32 {
            let args = [constant(index), scalar.clone()];
            assert!(projection_scalar(domain, opcode::BYTE, &args).is_none());
            assert_eq!(
                domain.observe_byte(opcode::BYTE, &args),
                domain.apply(opcode::BYTE, &args)
            );
        }
        let args = [scalar, constant(255)];
        assert!(projection_scalar(domain, opcode::AND, &args).is_none());
        assert_eq!(
            domain.observe_byte(opcode::AND, &args),
            domain.apply(opcode::AND, &args)
        );
    }
    let domain = product(8);
    for component in 0..3 {
        let mut numeric = NumericValue::top();
        let exact = U256::from(0x1122);
        match component {
            0 => numeric.bits = KnownBits::exact(exact),
            1 => numeric.interval = Interval::exact(exact),
            _ => numeric.congruence = Congruence::exact(exact),
        }
        let scalar = AbstractValue::from_numeric(numeric).with_symbolic_limit();
        for (op, args) in [
            (opcode::BYTE, [constant(30), scalar.clone()]),
            (opcode::AND, [scalar, constant(255)]),
        ] {
            assert!(projection_scalar(domain, op, &args).is_none());
            let actual = domain.observe_byte(op, &args);
            assert_eq!(actual, domain.apply(op, &args));
            assert!(actual.singleton().is_some());
            assert!(!actual.symbolic_limit_reached());
        }
    }
}

#[test]
fn disallowed_ops_arities_profiles_and_nonliteral_operands_keep_the_original_path() {
    let domain = product(8);
    let scalar = AbstractValue::unknown_address()
        .with_symbol(Symbol::Caller, Some(51))
        .with_symbolic_limit();
    let nonliteral = |literal: usize| {
        let mut numeric = NumericValue::top();
        numeric.bits = KnownBits::exact(U256::from(literal));
        AbstractValue::from_numeric(numeric)
    };
    let cases = [
        (opcode::ADD, vec![scalar.clone(), constant(255)]),
        (0xff, vec![scalar.clone(), constant(255)]),
        (opcode::NOT, vec![scalar.clone()]),
        (opcode::BYTE, vec![]),
        (opcode::BYTE, vec![constant(0)]),
        (opcode::BYTE, vec![constant(0), scalar.clone(), constant(0)]),
        (opcode::AND, vec![scalar.clone()]),
        (
            opcode::AND,
            vec![scalar.clone(), constant(255), constant(0)],
        ),
        (opcode::BYTE, vec![nonliteral(31), scalar.clone()]),
        (
            opcode::BYTE,
            vec![domain.join(&constant(0), &constant(31)), scalar.clone()],
        ),
        (
            opcode::BYTE,
            vec![AbstractValue::unknown_byte(), scalar.clone()],
        ),
        (opcode::AND, vec![scalar.clone(), nonliteral(255)]),
        (
            opcode::AND,
            vec![scalar.clone(), domain.join(&constant(254), &constant(255))],
        ),
        (opcode::AND, vec![scalar.clone(), constant(254)]),
        (opcode::AND, vec![scalar.clone(), constant(256)]),
        (
            opcode::AND,
            vec![scalar.clone(), AbstractValue::unknown_byte()],
        ),
        (opcode::AND, vec![constant(255), scalar.clone()]),
    ];
    for (op, args) in cases {
        assert!(projection_scalar(domain, op, &args).is_none());
        assert_eq!(domain.observe_byte(op, &args), domain.apply(op, &args));
    }
    let constants = Domain::new(NonZeroUsize::new(8).unwrap());
    for domain in [constants, Domain::default()] {
        for (op, args) in [
            (opcode::BYTE, [constant(31), scalar.clone()]),
            (opcode::AND, [scalar.clone(), constant(255)]),
        ] {
            assert!(projection_scalar(domain, op, &args).is_none());
            assert_eq!(domain.observe_byte(op, &args), domain.apply(op, &args));
        }
    }
}

#[test]
fn observations_keep_transfer_sources_and_markers_but_clear_result_aliases_and_roles() {
    let domain = product(8);
    let mut scalar = AbstractValue::unknown_address()
        .with_symbol(Symbol::Caller, Some(51))
        .with_identity(51, 3)
        .with_symbolic_limit();
    scalar.provenance = Provenance::source(Origin::Storage).with_code_address_role();
    let frozen = scalar.clone();
    for (op, args) in [
        (opcode::BYTE, [constant(31), scalar.clone()]),
        (opcode::AND, [scalar.clone(), constant(255)]),
    ] {
        let actual = domain.observe_byte(op, &args);
        assert_eq!(
            actual.provenance.origins(),
            &OriginSet::from_sources([Origin::Constant, Origin::Storage, Origin::Arithmetic])
                .unwrap()
        );
        assert_eq!(actual.provenance(), domain.apply(op, &args).provenance());
        assert!(!actual.provenance().is_code_address());
        assert!(actual.identity().input().is_none());
        assert!(!actual.same_identity(&scalar));
        assert!(actual.expression().is_none());
        assert!(actual.symbolic_limit_reached());
    }
    let mut suffix = NumericValue::top();
    suffix.bits = KnownBits::new(U256::from(0xffff ^ 0x1122), U256::from(0x1122)).unwrap();
    let suffix = scalar.clone().with_numeric(suffix);
    for (op, args, expected) in [
        (opcode::BYTE, [constant(30), suffix.clone()], 0x11),
        (opcode::BYTE, [constant(31), suffix.clone()], 0x22),
        (opcode::AND, [suffix, constant(255)], 0x22),
        (opcode::BYTE, [constant(0), scalar.clone()], 0),
    ] {
        assert!(projection_scalar(domain, op, &args).is_some());
        let actual = domain.observe_byte(op, &args);
        assert_eq!(
            actual.numeric(),
            &NumericValue::constant(U256::from(expected))
        );
        assert_eq!(actual, domain.apply(op, &args));
        assert!(!actual.symbolic_limit_reached());
        assert!(!actual.same_identity(&scalar));
    }
    let mut unmarked = frozen.clone();
    unmarked.symbolic_limit = false;
    let actual = domain.observe_byte(opcode::AND, &[unmarked.clone(), constant(255)]);
    assert!(!actual.symbolic_limit_reached());
    let actual = domain.observe_byte(
        opcode::AND,
        &[unmarked, constant(255).with_symbolic_limit()],
    );
    assert!(actual.symbolic_limit_reached());
    assert_eq!(scalar, frozen);
    assert!(scalar.same_identity(&frozen));
    assert_eq!(
        scalar.provenance.origins().sources(),
        Some(&BTreeSet::from([Origin::Storage]))
    );
}
