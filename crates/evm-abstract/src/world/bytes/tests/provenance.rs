//! Singleton values keep their observed source when stored in byte state.

use super::super::*;
use crate::domain::{NumericValue, identity::Symbol, provenance::Origin};

fn calldata_seven() -> TestValue {
    TestValue::constant(U256::from(7)).with_origin(Origin::Calldata)
}

fn assert_calldata_seven(value: TestValue) {
    assert_eq!(value.singleton(), Some(U256::from(7)));
    assert!(
        value
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Calldata),
        "singleton lost calldata source: {value:?}"
    );
}

fn assert_sources(value: &TestValue, expected: U256, origins: &[Origin]) {
    assert_eq!(value.numeric(), &NumericValue::constant(expected));
    for origin in origins {
        assert!(
            value
                .provenance()
                .origins()
                .sources()
                .unwrap()
                .contains(origin),
            "missing {origin:?}: {value:?}"
        );
    }
}

fn input(scope: u64) -> TestValue {
    let mut value = TestValue::top();
    value.set_origin(Origin::Calldata);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(NumericValue::constant(U256::from(7)))
}

#[test]
fn singleton_source_byte_roundtrip() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut memory = TestBytes::memory();
    memory
        .write_byte(&constant(0), &calldata_seven(), domain.borrowed())
        .unwrap();
    assert_calldata_seven(memory.read_byte(&constant(0), domain.borrowed()));
}

#[test]
fn singleton_source_word_roundtrip() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(0), &calldata_seven(), domain.borrowed())
        .unwrap();
    assert_calldata_seven(memory.read_word(&constant(0), domain.borrowed()));
}

#[test]
fn constant_offset_collision_copy_and_slice_keep_both_sources() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut source = TestBytes::memory();
    source
        .write_byte(&constant(7), &calldata_seven(), domain.borrowed())
        .unwrap();
    let old = source.read_byte(&constant(7), domain.borrowed());
    let snapshot = source.clone();
    let mut target = TestBytes::memory();
    target
        .write_byte(
            &constant(8),
            &constant(7).with_origin(Origin::Returndata),
            domain.borrowed(),
        )
        .unwrap();
    target
        .copy_from(
            &constant(7),
            &source,
            &constant(7),
            &constant(1),
            domain.borrowed(),
        )
        .unwrap();
    for index in [7, 8] {
        assert_sources(
            &target.byte_at(index, domain.borrowed()),
            U256::from(7),
            &[Origin::Calldata, Origin::Returndata],
        );
    }
    let slice = target
        .slice(&constant(7), &constant(2), domain.borrowed())
        .unwrap();
    assert_sources(
        &slice.byte_at(0, domain.borrowed()),
        U256::from(7),
        &[Origin::Calldata, Origin::Returndata],
    );
    assert_sources(&old, U256::from(7), &[Origin::Calldata]);
    assert_eq!(source, snapshot);
    assert_eq!(target.join(&target, domain.borrowed()), target);
}

#[test]
fn normalized_zero_and_exact_writes_keep_singleton_sources_through_numeric_projection() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut zero = TestBytes::memory();
    zero.write_byte(
        &constant(0),
        &constant(256).with_origin(Origin::Calldata),
        domain.borrowed(),
    )
    .unwrap();
    assert!(matches!(zero.state_root().kind(), ByteStateKind::Zero));
    let mut exact = TestBytes::exact(&[0x11; 32]);
    let root = exact.state_root().clone();
    let word = TestValue::constant(U256::from_be_bytes([0x11; 32])).with_origin(Origin::Calldata);
    exact
        .write_word(&constant(0), &word, domain.borrowed())
        .unwrap();
    exact
        .copy_from(
            &constant(0),
            &TestBytes::exact(&[0x11; 32]),
            &constant(0),
            &constant(32),
            domain.borrowed(),
        )
        .unwrap();
    let exact = exact
        .slice(&constant(0), &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(exact.state_root(), &root);
    for value in [&zero, &exact] {
        let expected = value.byte_at(0, domain.borrowed()).singleton().unwrap();
        assert_sources(
            &value.byte_at(0, domain.borrowed()),
            expected,
            &[Origin::Calldata],
        );
        let form = value.project(domain.borrowed());
        assert_sources(
            &form.byte_at(0, domain.borrowed()),
            expected,
            &[Origin::Calldata],
        );
        assert_eq!(form.state_root(), value.state_root());
        assert_eq!(form, form.project(domain.borrowed()));
    }
}

#[test]
fn singleton_inputs_never_gain_a_shared_identity_and_keep_source_records() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let a = input(711);
    let b = input(712);
    assert!(!a.same_identity(&b));
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(0), &a, domain.borrowed())
        .unwrap();
    let old = memory.read_word(&constant(0), domain.borrowed());
    let snapshot = memory.clone();
    memory
        .write_word(&constant(32), &b, domain.borrowed())
        .unwrap();
    for (offset, original) in [(0, &a), (32, &b)] {
        let read = memory.read_word(&constant(offset), domain.borrowed());
        assert_sources(&read, U256::from(7), &[Origin::Calldata]);
        if read.identity().input().is_some() {
            assert_eq!(read.identity().input(), original.identity().input());
        }
    }
    assert!(
        !memory
            .read_word(&constant(0), domain.borrowed())
            .same_identity(&memory.read_word(&constant(32), domain.borrowed()))
    );
    for value in [&memory, &memory.project(domain.borrowed())] {
        for original in [&a, &b] {
            assert_eq!(
                value.operands.get(original.expression().unwrap()),
                Some(original)
            );
        }
    }
    assert_sources(&old, U256::from(7), &[Origin::Calldata]);
    assert!(old.same_identity(&a));
    assert_eq!(
        snapshot
            .read_word(&constant(0), domain.borrowed())
            .provenance(),
        old.provenance()
    );
}

#[test]
fn singleton_fresh_history_survives_collision_projection_and_rename() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let mut memory = TestBytes::memory();
    memory
        .write_word(
            &constant(0),
            &calldata_seven().with_expression(a.clone()),
            domain.borrowed(),
        )
        .unwrap();
    memory
        .write_word(
            &constant(32),
            &constant(7)
                .with_origin(Origin::Returndata)
                .with_expression(b.clone()),
            domain.borrowed(),
        )
        .unwrap();
    let snapshot = memory.clone();
    let mut form = memory.project(domain.borrowed());
    assert!(
        form.fresh_leaves()
            .is_superset(&BTreeSet::from([a.clone(), b.clone()]))
    );
    let next = ExprId::fresh().unwrap();
    form.rename_fresh(
        &BTreeMap::from([(a.clone(), next.clone())]),
        limits(domain.borrowed()),
        domain.borrowed(),
    )
    .unwrap();
    assert!(!form.fresh_leaves().contains(&a));
    assert!(form.fresh_leaves().contains(&next));
    assert!(form.fresh_leaves().contains(&b));
    assert_eq!(memory, snapshot);
    assert_sources(
        &form.read_word(&constant(0), domain.borrowed()),
        U256::from(7),
        &[Origin::Calldata],
    );
    assert_sources(
        &form.read_word(&constant(32), domain.borrowed()),
        U256::from(7),
        &[Origin::Returndata],
    );
}

#[test]
fn rename_collision_unions_origins_and_refreshes_all_constant_components() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let first = input(731)
        .with_numeric(
            TestValue::unsigned_range(U256::from(6), U256::from(8))
                .unwrap()
                .numeric()
                .clone(),
        )
        .with_expression(a.clone());
    let second = input(732)
        .with_origin(Origin::Returndata)
        .with_numeric(
            TestValue::unsigned_range(U256::from(5), U256::from(9))
                .unwrap()
                .numeric()
                .clone(),
        )
        .with_expression(b.clone());
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(0), &first, domain.borrowed())
        .unwrap();
    memory
        .write_word(&constant(32), &second, domain.borrowed())
        .unwrap();
    let snapshot = memory.clone();
    memory
        .rename_fresh(
            &BTreeMap::from([
                (a, ExprId::constant(U256::from(7))),
                (b, ExprId::constant(U256::from(7))),
            ]),
            limits(domain.borrowed()),
            domain.borrowed(),
        )
        .unwrap();
    for current in [&memory, &memory.project(domain.borrowed())] {
        let fact = current
            .operands
            .get(&ExprId::constant(U256::from(7)))
            .unwrap();
        assert_sources(fact, U256::from(7), &[Origin::Calldata, Origin::Returndata]);
        assert!(fact.identity().input().is_none());
        for offset in [0, 32] {
            let read = current.read_word(&constant(offset), domain.borrowed());
            assert_sources(
                &read,
                U256::from(7),
                &[Origin::Calldata, Origin::Returndata],
            );
            assert!(read.identity().input().is_none());
        }
    }
    assert_eq!(
        snapshot
            .read_word(&constant(32), domain.borrowed())
            .numeric(),
        second.numeric()
    );
}
