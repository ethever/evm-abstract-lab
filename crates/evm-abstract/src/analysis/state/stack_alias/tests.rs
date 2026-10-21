use super::*;
use crate::domain::EvmNumericAlgebra as _;
use crate::domain::ValueAlgebra;
use crate::{
    analysis::machine::fixtures::fixture,
    domain::{ProductAlgebra, symbolic::ExprId},
};
use alloy_primitives::U256;

fn fresh() -> TestValue {
    TestValue::top().with_expression(ExprId::fresh().unwrap())
}

#[test]
fn form_uses_node_identity_and_uniform_bounds_only() {
    let x = fresh();
    let y = fresh();
    let stack = [x.clone(), y.clone(), x.clone(), y];
    assert_eq!(StackAliases::form(&stack, true).unwrap().0, [0, 1, 0, 1]);
    assert_eq!(StackAliases::form(&stack, false).unwrap().0, [0, 1, 2, 3]);

    // Equal numeric bounds and structurally equal, separately allocated input
    // nodes are deliberately insufficient evidence in this small prototype.
    let a = ExprId::constant(U256::from(7));
    let b = ExprId::constant(U256::from(7));
    assert_eq!(a, b);
    let separate = [
        TestValue::top().with_expression(a),
        TestValue::top().with_expression(b),
    ];
    assert_eq!(StackAliases::form(&separate, true).unwrap().0, [0, 1]);

    let narrow = x.clone().with_numeric(
        crate::domain::NumericValue::unsigned_range(U256::from(2), U256::from(9)).unwrap(),
    );
    let mixed = [x, narrow];
    let aliases = StackAliases::form(&mixed, true).unwrap();
    assert_eq!(aliases.0, [0, 1]);
    let scalar = mixed
        .iter()
        .map(|v| TestValue::from_numeric(v.numeric().clone()))
        .collect::<Vec<_>>();
    let restored = aliases.instantiate(&scalar, true).unwrap();
    for (old, new) in mixed.iter().zip(&restored) {
        assert_eq!(old.numeric(), new.numeric());
    }
    assert_ne!(restored[0].expression(), restored[1].expression());
}

#[test]
fn intersection_preserves_common_pairs_even_when_leaders_differ() {
    let left = StackAliases(vec![0, 0, 2, 2]);
    let right = StackAliases(vec![0, 1, 1, 1]);
    let intersection = left.intersect(&right).unwrap();
    assert_eq!(intersection.0, [0, 1, 2, 2]);
    assert_eq!(left.intersect(&left).unwrap(), left);
    assert_eq!(
        intersection
            .intersect(&StackAliases(vec![0, 1, 2, 3]))
            .unwrap()
            .0,
        [0, 1, 2, 3]
    );
    assert!(left.intersect(&StackAliases(vec![])).is_err());
}

#[test]
fn payload_join_and_widen_drop_only_unshared_guarantees() {
    use super::super::{form, instantiate, join, semantic_eq};
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    let x = fresh();
    let y = fresh();
    input.active_mut().stack = vec![x.clone(), x, y.clone(), y];
    let left = form(&input, &entry, domain).unwrap();
    let a = fresh();
    let b = fresh();
    input.active_mut().stack = vec![a, b.clone(), b.clone(), b];
    let right = form(&input, &entry, domain).unwrap();
    assert!(!semantic_eq(&left, &right));
    for widen in [false, true] {
        let joined = join(&left, &right, domain, widen).unwrap();
        assert_eq!(joined.active().stack_aliases.0, [0, 1, 2, 2]);
        let first = instantiate(&joined, domain).unwrap();
        let second = instantiate(&joined, domain).unwrap();
        let a = &first.active().stack;
        let b = &second.active().stack;
        assert_eq!(a[2].expression(), a[3].expression());
        assert_ne!(a[0].expression(), a[1].expression());
        assert_ne!(a[1].expression(), a[2].expression());
        assert_ne!(a[2].expression(), b[2].expression());
        assert!(first.relations.is_empty());
    }
    let disabled = left
        .active()
        .stack_aliases
        .instantiate(&left.active().stack, false)
        .unwrap();
    assert!(disabled.iter().all(|value| value.expression().is_none()));
}
