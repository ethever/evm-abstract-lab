//! Slot state retains source observations even when their numeric value is exact.

use super::super::*;
use crate::{
    Fork,
    domain::{
        NumericValue,
        identity::Symbol,
        provenance::Origin,
        symbolic::{ExprId, ExprLimits},
    },
    world::Account,
};

fn v(n: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn world(complete: bool) -> (World, Address) {
    let address = Address::repeat_byte(0x39);
    let mut world = World::offline(Fork::Osaka, "singleton sources", "leaf");
    world
        .insert(
            address,
            if complete {
                Account::empty()
            } else {
                Account::unknown()
            },
        )
        .unwrap();
    (world, address)
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

fn assert_sources(value: &AbstractValue, number: u64, origins: &[Origin]) {
    assert_eq!(value.numeric(), &NumericValue::constant(U256::from(number)));
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
fn singleton_source_persistent_slot_roundtrip() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store
        .write_checked(address, &v(4), &v(7).with_origin(Origin::Calldata), domain)
        .unwrap();
    assert_calldata_seven(store.read(address, &v(4), domain));
}

#[test]
fn singleton_source_transient_slot_roundtrip() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store
        .write_transient_checked(address, &v(4), &v(7).with_origin(Origin::Calldata), domain)
        .unwrap();
    assert_calldata_seven(store.read_transient(address, &v(4), domain));
}

#[test]
fn singleton_source_complete_initial_observation() {
    let (mut world, address) = world(true);
    world
        .accounts
        .get_mut(&address)
        .unwrap()
        .storage
        .insert(U256::from(4), v(7).with_origin(Origin::Calldata));
    assert_calldata_seven(Store::new(&world).read(address, &v(4), Domain::default()));
}

#[test]
fn singleton_source_unknown_initial_observation() {
    let (mut world, address) = world(false);
    world
        .accounts
        .get_mut(&address)
        .unwrap()
        .storage
        .insert(U256::from(4), v(7).with_origin(Origin::Calldata));
    assert_calldata_seven(Store::new(&world).read(address, &v(4), Domain::default()));
}

#[test]
fn equal_numeric_writes_keep_origins_without_overwriting_constant_key_facts() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store
        .write_checked(address, &v(7), &v(7).with_origin(Origin::Calldata), domain)
        .unwrap();
    let old = store.read(address, &v(7), domain);
    let snapshot = store.snapshot();
    store
        .write_checked(
            address,
            &v(8),
            &v(7).with_origin(Origin::Returndata),
            domain,
        )
        .unwrap();
    store
        .write_transient_checked(
            address,
            &v(7),
            &v(7).with_origin(Origin::Environment),
            domain,
        )
        .unwrap();
    for key in [7, 8] {
        assert_sources(
            &store.read(address, &v(key), domain),
            7,
            &[Origin::Calldata, Origin::Returndata],
        );
    }
    assert_sources(
        &store.read_transient(address, &v(7), domain),
        7,
        &[Origin::Environment],
    );
    assert_sources(&old, 7, &[Origin::Calldata]);
    assert!(
        !old.provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Returndata)
    );
    assert_eq!(store.join(&store, domain), store);
    store.restore(snapshot);
    assert_eq!(
        store.read(address, &v(7), domain).provenance(),
        old.provenance()
    );
    assert_eq!(
        store.read_transient(address, &v(7), domain).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn colliding_initial_singletons_and_normalized_zero_keep_sources_in_widening() {
    let domain = Domain::default();
    for complete in [true, false] {
        let (mut world, address) = world(complete);
        for (key, number, origin) in [
            (7, 7, Origin::Calldata),
            (8, 7, Origin::Environment),
            (9, 0, Origin::Returndata),
        ] {
            world
                .accounts
                .get_mut(&address)
                .unwrap()
                .storage
                .insert(U256::from(key), v(number).with_origin(origin));
        }
        let store = Store::new(&world);
        let form = store.widening_form(domain, ExprLimits::default()).unwrap();
        for current in [&store, &form] {
            for key in [7, 8] {
                assert_sources(
                    &current.read(address, &v(key), domain),
                    7,
                    &[Origin::Calldata, Origin::Environment],
                );
            }
            assert_sources(
                &current.read(address, &v(9), domain),
                0,
                &[Origin::Returndata],
            );
        }
        assert_eq!(
            form.slot_root(address, false).unwrap(),
            store.slot_root(address, false).unwrap()
        );
        assert_eq!(
            form,
            form.widening_form(domain, ExprLimits::default()).unwrap()
        );
    }
}

#[test]
fn independent_singleton_inputs_remain_separate_in_source_metadata() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let a = input(721);
    let b = input(722);
    assert!(!a.same_identity(&b));
    let mut store = Store::new(&world);
    for transient in [false, true] {
        for (key, value) in [(7, &a), (8, &b)] {
            if transient {
                store
                    .write_transient_checked(address, &v(key), value, domain)
                    .unwrap();
            } else {
                store
                    .write_checked(address, &v(key), value, domain)
                    .unwrap();
            }
        }
    }
    let form = store.widening_form(domain, ExprLimits::default()).unwrap();
    for current in [&store, &form] {
        for transient in [false, true] {
            let facts = current
                .symbolic_facts(transient)
                .collect::<BTreeMap<_, _>>();
            for original in [&a, &b] {
                assert_eq!(facts.get(original.expression().unwrap()), Some(&original));
            }
            for (key, original) in [(7, &a), (8, &b)] {
                let read = if transient {
                    current.read_transient(address, &v(key), domain)
                } else {
                    current.read(address, &v(key), domain)
                };
                assert_sources(&read, 7, &[Origin::Calldata]);
                assert!(read.same_identity(original));
            }
        }
    }
}

#[test]
fn singleton_guard_projection_retains_whole_case_numbers_sources_and_roots() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut a = Store::new(&world);
    let mut b = a.clone();
    a.write_checked(address, &v(0), &v(7).with_origin(Origin::Calldata), domain)
        .unwrap();
    b.write_checked(
        address,
        &v(0),
        &v(9).with_origin(Origin::Returndata),
        domain,
    )
    .unwrap();
    let selector = ExprId::fresh().unwrap();
    let joined = a
        .join_guarded(&b, domain, selector.clone(), ExprLimits::default())
        .unwrap();
    for (truth, direct, number, origin) in [
        (U256::ONE, &a, 7, Origin::Calldata),
        (U256::ZERO, &b, 9, Origin::Returndata),
    ] {
        let mut projected = joined.clone();
        projected
            .rename_fresh(
                &BTreeMap::from([(selector.clone(), ExprId::constant(truth))]),
                ExprLimits::default(),
            )
            .unwrap();
        let form = projected
            .widening_form(domain, ExprLimits::default())
            .unwrap();
        assert_eq!(
            form.slot_root(address, false).unwrap(),
            direct.slot_root(address, false).unwrap()
        );
        assert_sources(&form.read(address, &v(0), domain), number, &[origin]);
    }
    assert!(joined.read(address, &v(0), domain).contains(U256::from(7)));
    assert!(joined.read(address, &v(0), domain).contains(U256::from(9)));
}

#[test]
fn renamed_slot_fact_collisions_keep_both_origins_without_input_aliases() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let first = input(741).with_expression(a.clone());
    let second = input(742)
        .with_origin(Origin::Returndata)
        .with_expression(b.clone());
    let mut store = Store::new(&world);
    for transient in [false, true] {
        for (key, value) in [(7, &first), (8, &second)] {
            if transient {
                store
                    .write_transient_checked(address, &v(key), value, domain)
                    .unwrap();
            } else {
                store
                    .write_checked(address, &v(key), value, domain)
                    .unwrap();
            }
        }
    }
    let snapshot = store.snapshot();
    store
        .rename_fresh(
            &BTreeMap::from([
                (a, ExprId::constant(U256::from(7))),
                (b, ExprId::constant(U256::from(7))),
            ]),
            ExprLimits::default(),
        )
        .unwrap();
    let form = store.widening_form(domain, ExprLimits::default()).unwrap();
    for current in [&store, &form] {
        for transient in [false, true] {
            let facts = current
                .symbolic_facts(transient)
                .collect::<BTreeMap<_, _>>();
            let fact = facts.get(&ExprId::constant(U256::from(7))).unwrap();
            assert_sources(fact, 7, &[Origin::Calldata, Origin::Returndata]);
            assert!(fact.identity().input().is_none());
            for key in [7, 8] {
                let read = if transient {
                    current.read_transient(address, &v(key), domain)
                } else {
                    current.read(address, &v(key), domain)
                };
                assert_sources(&read, 7, &[Origin::Calldata, Origin::Returndata]);
                assert!(read.identity().input().is_none());
            }
        }
    }
    store.restore(snapshot);
    assert!(store.read(address, &v(7), domain).same_identity(&first));
    assert!(
        store
            .read_transient(address, &v(8), domain)
            .same_identity(&second)
    );
}

#[test]
fn singleton_metadata_bound_and_cancellation_reject_slot_writes_atomically() {
    let (world, address) = world(true);
    let domain = Domain::default();
    let mut store = Store::new(&world);
    store
        .write_checked(address, &v(0), &v(0).with_origin(Origin::Calldata), domain)
        .unwrap();
    let small = Domain::from_spec(domain.spec().with_relations(
        crate::domain::relational::RelationLimits {
            max_nodes: 2,
            max_depth: 2,
            ..domain.spec().relations()
        },
    ));
    store
        .write_checked(address, &v(13).with_origin(Origin::Address), &v(0), small)
        .unwrap();
    let snapshot = store.clone();
    assert!(matches!(
        store.write_checked(address, &v(14).with_origin(Origin::Address), &v(0), small),
        Err(StoreError::Expression(
            crate::domain::symbolic::ExprError::Limit { .. }
        ))
    ));
    assert_eq!(store, snapshot);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(
            store
                .write_checked(
                    address,
                    &v(0),
                    &v(7).with_origin(Origin::Returndata),
                    domain
                )
                .is_err()
        );
    });
    assert_eq!(store, snapshot);
    assert_sources(&store.read(address, &v(0), domain), 0, &[Origin::Calldata]);
}
