//! 封装常量组件后，核对组合值、事实输入及原有输出合同。

use evm_abstract::{
    U256,
    domain::{
        AbstractValue, Domain, DomainSpec, Profile,
        facts::{FactError, FiniteSet, Symbol, UnaryPredicate},
        provenance::Origin,
    },
};
use std::{collections::BTreeSet, num::NonZeroUsize};

fn domain(profile: Profile, capacity: usize, fact_limit: usize) -> Domain {
    Domain::from_spec(DomainSpec::new(
        profile,
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(fact_limit).unwrap(),
    ))
}

fn constants(values: &[u64]) -> BTreeSet<U256> {
    values.iter().copied().map(U256::from).collect()
}

fn membership(values: &[u64]) -> UnaryPredicate {
    UnaryPredicate::MemberOf(FiniteSet::new(constants(values)).unwrap())
}

#[test]
fn a_top_constant_component_does_not_erase_byte_constraints() {
    let byte = AbstractValue::unknown_byte();
    assert!(byte.finite_constants().is_top());
    assert_eq!(byte.finite_constants().as_values(), byte.constants());
    assert!(byte.constants().is_none());
    assert_ne!(byte, AbstractValue::top());
    assert!(byte.contains(U256::ZERO));
    assert!(byte.contains(U256::from(255)));
    assert!(!byte.contains(U256::from(256)));
    assert!(serde_json::to_value(&byte).unwrap().is_object());
}

#[test]
fn projecting_over_capacity_forgets_only_the_constant_component() {
    let wide = domain(Profile::Product, 3, 256);
    let input = [5_u64, 8]
        .into_iter()
        .fold(AbstractValue::constant(U256::from(2)), |old, next| {
            wide.join(&old, &AbstractValue::constant(U256::from(next)))
        });
    let input = input.with_origin(Origin::Storage);
    assert_eq!(
        input.finite_constants().as_values(),
        Some(&constants(&[2, 5, 8]))
    );

    let projected = domain(Profile::Product, 1, 256).project(&input);
    assert!(projected.finite_constants().is_top());
    assert_eq!(projected.known_bits(), input.known_bits());
    assert_eq!(projected.interval(), input.interval());
    assert_eq!(projected.congruence(), input.congruence());
    assert_eq!(projected.provenance(), input.provenance());
    assert_eq!(serde_json::to_value(&projected).unwrap()["nonzero"], true);
    for member in [2_u64, 5, 8] {
        assert!(projected.contains(U256::from(member)));
    }
    for excluded in [0_u64, 1, 3, 4, 6, 7, 9, 16] {
        assert!(!projected.contains(U256::from(excluded)));
    }
    let constants_only = domain(Profile::ConstantsOnly, 1, 256).project(&input);
    assert_eq!(constants_only.numeric(), AbstractValue::top().numeric());
    assert_eq!(constants_only.provenance(), input.provenance());
}

#[test]
fn fact_membership_uses_fact_capacity_before_domain_projection() {
    let input = membership(&[2, 5, 8]);
    let projected = domain(Profile::Product, 1, 256)
        .from_facts(std::slice::from_ref(&input))
        .unwrap();
    assert!(projected.finite_constants().is_top());
    assert_eq!(
        projected.interval().unsigned_bounds(),
        (U256::from(2), U256::from(8))
    );
    assert_eq!(
        projected.congruence().modulus_residue(),
        Some((U256::from(3), U256::from(2)))
    );
    assert!(!projected.may_be_zero());
    assert!(projected.contains(U256::from(5)));
    assert!(!projected.contains(U256::from(6)));
    assert_eq!(
        domain(Profile::ConstantsOnly, 1, 256)
            .from_facts(std::slice::from_ref(&input))
            .unwrap(),
        AbstractValue::top()
    );
    assert_eq!(
        domain(Profile::Product, 3, 2).from_facts(&[input]),
        Err(FactError::Capacity {
            max_atoms: 2,
            required: 3,
        })
    );
}

#[test]
fn contradictory_finite_intersections_remain_errors() {
    let domain = domain(Profile::Product, 1, 256);
    assert_eq!(
        domain.from_facts(&[membership(&[2, 5, 8]), membership(&[3, 6, 9])]),
        Err(FactError::Contradiction {
            subject: Symbol::THIS,
        })
    );
    assert_eq!(
        domain.from_facts(&[membership(&[0]), UnaryPredicate::NonZero]),
        Err(FactError::Contradiction {
            subject: Symbol::THIS,
        })
    );
}

#[test]
fn finite_queries_and_output_keep_the_existing_value_contract() {
    let domain = domain(Profile::Product, 2, 256);
    let value = domain.join(
        &AbstractValue::constant(U256::from(8)),
        &AbstractValue::constant(U256::from(16)),
    );
    let expected = constants(&[8, 16]);
    assert_eq!(value.finite_constants().as_values(), Some(&expected));
    assert_eq!(value.constants(), Some(&expected));
    assert_eq!(value.singleton(), None);
    assert!(value.contains(U256::from(8)));
    assert!(value.contains(U256::from(16)));
    assert!(!value.contains(U256::from(12)));
    assert_eq!(value.to_string(), "{0x8, 0x10}");
    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(json["Constants"], serde_json::to_value(expected).unwrap());
    assert_eq!(json.as_object().unwrap().len(), 6);
    assert!(json.get("finite").is_none());
    assert_eq!(serde_json::to_value(AbstractValue::top()).unwrap(), "Top");
}
