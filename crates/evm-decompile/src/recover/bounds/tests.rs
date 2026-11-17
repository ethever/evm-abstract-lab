//! Counterexamples for mathematical bounds over modular stack operations.
use super::{AbstractValue, Bounds, Cell, EntityId, Span, U256};
use evm_domain::identity::Symbol;

fn cell(id: u64, value: U256) -> Cell {
    Cell {
        value: EntityId(id),
        abstract_value: AbstractValue::constant(value),
        control: None,
    }
}
fn bounded(stack: &[Cell]) -> Bounds {
    Bounds {
        ranges: stack.iter().map(|c| Span::of(&c.abstract_value)).collect(),
        ..Bounds::default()
    }
}

#[test]
fn wrapping_add_is_not_an_ordinary_integer_increment() {
    for (pointer, step) in [(U256::MAX, U256::from(1)), (U256::from(1), U256::MAX)] {
        let stack = vec![cell(1, pointer), cell(2, pointer), cell(3, step)];
        let mut proof = bounded(&stack);
        proof.insert((1, 0, U256::from(1)), U256::ZERO);
        let args = vec![stack[2].clone(), stack[1].clone()];
        let output = cell(4, U256::ZERO);
        proof.step(0x01, &args, &[Some(step), Some(pointer)], Some(&output));
        assert!(proof.pairs.is_empty());
        assert_eq!(
            proof.ranges[1],
            Span {
                low: U256::ZERO,
                high: U256::ZERO
            }
        );
    }
}

#[test]
fn a_duplicate_survives_swaps_and_removal_of_the_original_binding() {
    let stack = vec![cell(1, U256::from(2)), cell(2, U256::from(12))];
    let mut proof = bounded(&stack);
    proof.insert((1, 0, U256::from(4)), U256::from(4));
    proof.duplicate(1); // C, P, copied P
    proof.swap(0, 1); // P, C, copied P
    proof.swap(0, 2); // copied P, C, original P
    proof.step(0x50, &[stack[1].clone()], &[], None);
    assert_eq!(
        proof.pairs.get(&(0, 1, U256::from(4))),
        Some(&U256::from(4))
    );
    assert_eq!(
        proof.ranges,
        vec![
            Span::of(&stack[1].abstract_value),
            Span::of(&stack[0].abstract_value)
        ]
    );
}

#[test]
fn immutable_input_constraints_have_scope_and_join_by_union() {
    let input =
        |scope| AbstractValue::top().with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope));
    let a = *input(1).identity().input().unwrap();
    let b = *input(2).identity().input().unwrap();
    let mut left = Bounds::default();
    left.remember_input(
        Some(a),
        Span {
            low: U256::ZERO,
            high: U256::from(255),
        },
    );
    assert!(!left.inputs.contains_key(&b));
    let mut right = Bounds::default();
    right.remember_input(
        Some(a),
        Span {
            low: U256::from(512),
            high: U256::from(1023),
        },
    );
    let joined = Bounds::joined((&left, &[]), (&right, &[]), &[], &[], false, false);
    assert_eq!(
        joined.inputs[&a],
        Span {
            low: U256::ZERO,
            high: U256::from(1023)
        }
    );
    let widened = Bounds::joined((&left, &[]), (&right, &[]), &[], &[], true, false);
    assert!(!widened.inputs.contains_key(&a));
    let unknown = Bounds::default();
    assert!(
        Bounds::joined((&left, &[]), (&unknown, &[]), &[], &[], false, false)
            .inputs
            .is_empty()
    );
}
