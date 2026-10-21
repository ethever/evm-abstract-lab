use super::super::{DomainSpec, EvmValue, ProductAlgebra, ValueAlgebra};
use crate::domain::NumericValue;
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

fn domain() -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(512).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    )))
}

#[test]
fn a_word_counter_stabilizes_before_enumerating_512_constants_or_256_bit_prefixes() {
    let domain = domain();
    let mut value = EvmValue::<NumericValue>::constant(U256::ZERO);
    let mut stable = false;
    for update in 0..12 {
        let next = domain.apply(
            opcode::ADD,
            &[value.clone(), EvmValue::<NumericValue>::constant(U256::ONE)],
        );
        let joined = domain.join(&value, &next);
        let widened = if update >= domain.spec().widening_after_updates() {
            domain.widen_procedure(&value, &joined)
        } else {
            joined
        };
        for word in 0..=update + 1 {
            assert!(widened.contains(U256::from(word)));
        }
        if value.numeric() == widened.numeric() {
            stable = true;
            break;
        }
        value = widened;
    }
    assert!(stable, "counter still growing after 12 updates: {value}");
    assert!(value.contains(U256::from(1024)));
}

#[test]
fn expanding_high_bits_keep_the_fixed_low_160_bit_target() {
    let domain = domain();
    let target = U256::from(0x101);
    let value = |high| EvmValue::<NumericValue>::constant((U256::from(high) << 160usize) | target);
    let old = [1_u64, 2, 3]
        .into_iter()
        .map(value)
        .reduce(|left, right| domain.join(&left, &right))
        .unwrap();
    let next = domain.join(&old, &value(4));
    let widened = domain.widen_procedure(&old, &next);
    for high in [1_u64, 2, 3, 4, 5, 1024] {
        assert!(widened.contains((U256::from(high) << 160usize) | target));
    }
    assert_eq!(
        domain.address_projection(&widened).singleton(),
        Some(target)
    );
    assert!(!widened.contains((U256::from(1024) << 160usize) | (target + U256::ONE)));
}

#[test]
fn small_finite_unions_and_unchanged_words_keep_all_facts() {
    let domain = domain();
    for values in [[0_u64, 1, 2], [5, 9, 9]] {
        let old = EvmValue::<NumericValue>::constant(U256::from(values[0]));
        let joined = values
            .into_iter()
            .map(|value| EvmValue::<NumericValue>::constant(U256::from(value)))
            .reduce(|left, right| domain.join(&left, &right))
            .unwrap();
        assert_eq!(
            domain.widen_procedure(&old, &joined).numeric(),
            joined.numeric()
        );
        assert_eq!(
            domain.widen_procedure(&joined, &joined).numeric(),
            joined.numeric()
        );
    }
}
