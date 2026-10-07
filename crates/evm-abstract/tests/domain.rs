//! 抽象域的代数性质独立于工作表实现；若 join 不满足这些性质，调度顺序会改变答案。

use evm_abstract::{
    U256,
    domain::{Domain, Value},
};
use proptest::{arbitrary::any, collection::vec, prop_assert, prop_assert_eq, proptest};
use std::num::NonZeroUsize;

fn set(domain: Domain, values: &[u16]) -> Value {
    values[1..]
        .iter()
        .fold(Value::constant(U256::from(values[0])), |old, v| {
            domain.join(&old, &Value::constant(U256::from(*v)))
        })
}

proptest! {
    #[test]
    fn joins_form_a_semilattice(a in vec(any::<u16>(), 1..12), b in vec(any::<u16>(), 1..12), c in vec(any::<u16>(), 1..12), cap in 1usize..10) {
        let d = Domain::new(NonZeroUsize::new(cap).unwrap());
        let (a, b, c) = (set(d, &a), set(d, &b), set(d, &c));
        prop_assert_eq!(d.join(&a, &a), a.clone());
        prop_assert_eq!(d.join(&a, &b), d.join(&b, &a));
        prop_assert_eq!(d.join(&d.join(&a, &b), &c), d.join(&a, &d.join(&b, &c)));
        prop_assert_eq!(d.join(&a, &Value::top()), Value::top());
    }

    #[test]
    fn arithmetic_transfer_covers_every_cartesian_input(a in vec(any::<u16>(), 1..5), b in vec(any::<u16>(), 1..5)) {
        let d = Domain::default();
        let (left, right) = (set(d, &a), set(d, &b));
        let sum = d.apply(0x01, &[left, right]);
        for x in &a { for y in &b { prop_assert!(sum.contains(U256::from(*x).wrapping_add(U256::from(*y)))); } }
    }
}

#[test]
fn capacity_promotes_to_top_instead_of_dropping_constants() {
    let d = Domain::new(NonZeroUsize::new(1).unwrap());
    let joined = d.join(
        &Value::constant(U256::from(3)),
        &Value::constant(U256::from(7)),
    );
    assert_eq!(joined.numeric(), Value::top().numeric());
    assert_eq!(
        joined.provenance(),
        Value::constant(U256::ZERO).provenance()
    );
    assert!(joined.contains(U256::from(3)) && joined.contains(U256::from(7)));
}

#[test]
fn arithmetic_uses_evm_operand_order_and_modular_words() {
    let d = Domain::default();
    let v = |x: u64| Value::constant(U256::from(x));
    assert!(d.apply(0x03, &[v(3), v(2)]).contains(U256::from(1)));
    assert!(d.apply(0x03, &[v(2), v(3)]).contains(U256::MAX));
    assert!(d.apply(0x04, &[v(7), v(0)]).contains(U256::ZERO));
    assert!(d.apply(0x1b, &[v(256), v(1)]).contains(U256::ZERO));
    assert!(
        d.apply(0x1d, &[v(256), Value::constant(U256::MAX)])
            .contains(U256::MAX)
    );
    assert!(
        d.apply(0x08, &[Value::constant(U256::MAX), v(1), v(17)])
            .contains(U256::from(1))
    );
}

#[test]
fn unknown_comparisons_still_return_only_boolean_values() {
    let boolean = Domain::default().apply(0x14, &[Value::top(), Value::constant(U256::ZERO)]);
    assert_eq!(boolean.constants().unwrap().len(), 2);
    assert!(boolean.contains(U256::ZERO) && boolean.contains(U256::from(1)));
    assert!(!boolean.contains(U256::from(2)));
}
