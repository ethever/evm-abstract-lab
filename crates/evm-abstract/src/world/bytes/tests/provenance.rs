//! Singleton values keep their observed source when stored in byte state.

use super::super::*;
use crate::domain::{NumericValue, identity::Symbol, provenance::Origin};

fn calldata_seven() -> AbstractValue {
    AbstractValue::constant(U256::from(7)).with_origin(Origin::Calldata)
}

fn assert_calldata_seven(value: AbstractValue) {
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

fn assert_sources(value: &AbstractValue, expected: U256, origins: &[Origin]) {
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

fn input(scope: u64) -> AbstractValue {
    let mut value = AbstractValue::top();
    value.set_origin(Origin::Calldata);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(NumericValue::constant(U256::from(7)))
}

#[test]
fn singleton_source_byte_roundtrip() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .write_byte(&constant(0), &calldata_seven(), 32, domain)
        .unwrap();
    assert_calldata_seven(memory.read_byte(&constant(0), domain));
}

#[test]
fn singleton_source_word_roundtrip() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(0), &calldata_seven(), 32, domain)
        .unwrap();
    assert_calldata_seven(memory.read_word(&constant(0), domain));
}

#[test]
fn constant_offset_collision_copy_and_slice_keep_both_sources() {
    let domain = Domain::default();
    let mut source = ByteArray::memory();
    source
        .write_byte(&constant(7), &calldata_seven(), 64, domain)
        .unwrap();
    let old = source.read_byte(&constant(7), domain);
    let snapshot = source.clone();
    let mut target = ByteArray::memory();
    target
        .write_byte(
            &constant(8),
            &constant(7).with_origin(Origin::Returndata),
            64,
            domain,
        )
        .unwrap();
    target
        .copy_from(
            &constant(7),
            &source,
            &constant(7),
            &constant(1),
            64,
            domain,
        )
        .unwrap();
    for index in [7, 8] {
        assert_sources(
            &target.byte_at(index, domain),
            U256::from(7),
            &[Origin::Calldata, Origin::Returndata],
        );
    }
    let slice = target
        .slice(&constant(7), &constant(2), 64, domain)
        .unwrap();
    assert_sources(
        &slice.byte_at(0, domain),
        U256::from(7),
        &[Origin::Calldata, Origin::Returndata],
    );
    assert_sources(&old, U256::from(7), &[Origin::Calldata]);
    assert_eq!(source, snapshot);
    assert_eq!(target.join(&target, domain), target);
}

#[test]
fn normalized_zero_and_exact_writes_keep_singleton_sources_through_widening() {
    let domain = Domain::default();
    let mut zero = ByteArray::memory();
    zero.write_byte(
        &constant(0),
        &constant(256).with_origin(Origin::Calldata),
        32,
        domain,
    )
    .unwrap();
    assert!(matches!(zero.state_root().kind(), ByteStateKind::Zero));
    let mut exact = ByteArray::exact(&[0x11; 32]);
    let root = exact.state_root().clone();
    let word =
        AbstractValue::constant(U256::from_be_bytes([0x11; 32])).with_origin(Origin::Calldata);
    exact.write_word(&constant(0), &word, 32, domain).unwrap();
    exact
        .copy_from(
            &constant(0),
            &ByteArray::exact(&[0x11; 32]),
            &constant(0),
            &constant(32),
            32,
            domain,
        )
        .unwrap();
    let exact = exact
        .slice(&constant(0), &constant(32), 32, domain)
        .unwrap();
    assert_eq!(exact.state_root(), &root);
    for value in [&zero, &exact] {
        let expected = value.byte_at(0, domain).singleton().unwrap();
        assert_sources(&value.byte_at(0, domain), expected, &[Origin::Calldata]);
        let form = value
            .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
            .unwrap();
        assert_sources(&form.byte_at(0, domain), expected, &[Origin::Calldata]);
        assert_eq!(form.state_root(), value.state_root());
        assert_eq!(
            form,
            form.widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
                .unwrap()
        );
    }
}

#[test]
fn singleton_inputs_never_gain_a_shared_identity_and_keep_source_records() {
    let domain = Domain::default();
    let a = input(711);
    let b = input(712);
    assert!(!a.same_identity(&b));
    let mut memory = ByteArray::memory();
    memory.write_word(&constant(0), &a, 64, domain).unwrap();
    let old = memory.read_word(&constant(0), domain);
    let snapshot = memory.clone();
    memory.write_word(&constant(32), &b, 64, domain).unwrap();
    for (offset, original) in [(0, &a), (32, &b)] {
        let read = memory.read_word(&constant(offset), domain);
        assert_sources(&read, U256::from(7), &[Origin::Calldata]);
        if read.identity().input().is_some() {
            assert_eq!(read.identity().input(), original.identity().input());
        }
    }
    assert!(
        !memory
            .read_word(&constant(0), domain)
            .same_identity(&memory.read_word(&constant(32), domain))
    );
    for value in [
        &memory,
        &memory
            .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
            .unwrap(),
    ] {
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
        snapshot.read_word(&constant(0), domain).provenance(),
        old.provenance()
    );
}

#[test]
fn singleton_fresh_history_survives_collision_widening_and_rename() {
    let domain = Domain::default();
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let mut memory = ByteArray::memory();
    memory
        .write_word(
            &constant(0),
            &calldata_seven().with_expression(a.clone()),
            64,
            domain,
        )
        .unwrap();
    memory
        .write_word(
            &constant(32),
            &constant(7)
                .with_origin(Origin::Returndata)
                .with_expression(b.clone()),
            64,
            domain,
        )
        .unwrap();
    let snapshot = memory.clone();
    let mut form = memory
        .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
        .unwrap();
    assert!(
        form.fresh_leaves()
            .is_superset(&BTreeSet::from([a.clone(), b.clone()]))
    );
    let next = ExprId::fresh().unwrap();
    form.rename_fresh(
        &BTreeMap::from([(a.clone(), next.clone())]),
        limits(crate::domain::ValueAlgebra::new(&domain)),
    )
    .unwrap();
    assert!(!form.fresh_leaves().contains(&a));
    assert!(form.fresh_leaves().contains(&next));
    assert!(form.fresh_leaves().contains(&b));
    assert_eq!(memory, snapshot);
    assert_sources(
        &form.read_word(&constant(0), domain),
        U256::from(7),
        &[Origin::Calldata],
    );
    assert_sources(
        &form.read_word(&constant(32), domain),
        U256::from(7),
        &[Origin::Returndata],
    );
}

#[test]
fn rename_collision_unions_origins_and_refreshes_all_constant_components() {
    let domain = Domain::default();
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let first = input(731)
        .with_numeric(
            AbstractValue::unsigned_range(U256::from(6), U256::from(8))
                .unwrap()
                .numeric()
                .clone(),
        )
        .with_expression(a.clone());
    let second = input(732)
        .with_origin(Origin::Returndata)
        .with_numeric(
            AbstractValue::unsigned_range(U256::from(5), U256::from(9))
                .unwrap()
                .numeric()
                .clone(),
        )
        .with_expression(b.clone());
    let mut memory = ByteArray::memory();
    memory.write_word(&constant(0), &first, 64, domain).unwrap();
    memory
        .write_word(&constant(32), &second, 64, domain)
        .unwrap();
    let snapshot = memory.clone();
    memory
        .rename_fresh(
            &BTreeMap::from([
                (a, ExprId::constant(U256::from(7))),
                (b, ExprId::constant(U256::from(7))),
            ]),
            limits(crate::domain::ValueAlgebra::new(&domain)),
        )
        .unwrap();
    for current in [
        &memory,
        &memory
            .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
            .unwrap(),
    ] {
        let fact = current
            .operands
            .get(&ExprId::constant(U256::from(7)))
            .unwrap();
        assert_sources(fact, U256::from(7), &[Origin::Calldata, Origin::Returndata]);
        assert!(fact.identity().input().is_none());
        for offset in [0, 32] {
            let read = current.read_word(&constant(offset), domain);
            assert_sources(
                &read,
                U256::from(7),
                &[Origin::Calldata, Origin::Returndata],
            );
            assert!(read.identity().input().is_none());
        }
    }
    assert_eq!(
        snapshot.read_word(&constant(32), domain).numeric(),
        second.numeric()
    );
}

#[test]
fn repeated_singleton_source_overwrite_drops_obsolete_fresh_records_in_widening() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let mut previous = None;
    for _ in 0..24 {
        let source = ExprId::fresh().unwrap();
        memory
            .write_word(
                &constant(0),
                &calldata_seven().with_expression(source.clone()),
                32,
                domain,
            )
            .unwrap();
        memory = memory
            .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
            .unwrap();
        assert_eq!(memory.fresh_leaves(), BTreeSet::from([source.clone()]));
        assert!(memory.work_size() < 500);
        assert_sources(
            &memory.read_word(&constant(0), domain),
            U256::from(7),
            &[Origin::Calldata],
        );
        if let Some(previous) = previous {
            assert!(!memory.operands.contains_key(&previous));
        }
        let saved = memory.clone();
        memory
            .write_word(
                &constant(0),
                &calldata_seven().with_expression(source.clone()),
                32,
                domain,
            )
            .unwrap();
        assert_eq!(
            memory
                .widening_form(domain, limits(crate::domain::ValueAlgebra::new(&domain)))
                .unwrap(),
            saved
        );
        previous = Some(source);
    }
}
