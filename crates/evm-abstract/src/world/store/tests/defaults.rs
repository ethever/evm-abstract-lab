//! Effective defaults survive ordinary joins, summary replay and loop layout.

use super::super::*;
use {crate::Fork, crate::world::test_support::TestAccount};

fn word(value: u64) -> TestValue {
    TestValue::constant(U256::from(value))
}

fn flags(store: &TestStore, owner: Address) -> [Option<bool>; 3] {
    [
        store.created_in_transaction(owner),
        store.pending_destruction(owner),
        store.touched_in_transaction(owner),
    ]
}

#[test]
fn unknown_lifecycle_defaults_agree_across_store_join_and_summary_replay() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let protected = Address::repeat_byte(0x71);
    let created = Address::repeat_byte(0x72);
    let destroyed = Address::repeat_byte(0x73);
    let one_path = Address::repeat_byte(0x74);
    let omitted = Address::repeat_byte(0x75);
    let mut world = TestWorld::new(Fork::Osaka, "default-aware lifecycle joins");
    world
        .insert(protected, TestAccount::from_hex("00", Fork::Osaka).unwrap())
        .unwrap();
    let mut left = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    left.unknown_effects(&BTreeSet::from([protected]), None, false, domain.borrowed())
        .unwrap();
    let mut right = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    for store in [&mut left, &mut right] {
        store.begin_creation(created);
        store.begin_creation(destroyed);
        store.selfdestruct(destroyed, destroyed, domain.borrowed());
    }
    left.begin_creation(one_path);
    let ordinary = left.join(&right, domain.borrowed());
    assert!(!ordinary.symbolic_limit_reached());
    let left_summary = left.summary_projection(domain.borrowed()).unwrap();
    let right_summary = right.summary_projection(domain.borrowed()).unwrap();
    for widen in [false, true] {
        let summary = left_summary.join(&right_summary, domain.borrowed(), widen);
        let replay = summary.instantiate(domain.borrowed()).unwrap();
        for store in [&ordinary, &replay] {
            assert_eq!(
                [
                    store.created_default(),
                    store.pending_destruction_default(),
                    store.touched_default(),
                ],
                [None; 3]
            );
            for (owner, expected) in [
                (protected, [Some(false), Some(false), None]),
                (created, [Some(true), Some(false), Some(true)]),
                (destroyed, [Some(true); 3]),
                (one_path, [None, Some(false), None]),
                (omitted, [None; 3]),
            ] {
                assert_eq!(flags(store, owner), expected);
            }
        }
        let mut restored = replay.clone();
        let checkpoint = restored.snapshot();
        restored.begin_creation(omitted);
        restored.selfdestruct(omitted, omitted, domain.borrowed());
        restored.restore(checkpoint);
        assert_eq!(restored, replay);
    }
}

#[test]
fn layout_only_coordinates_keep_input_defaults_after_other_coordinates_are_forgotten() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let retained = Address::repeat_byte(0x81);
    let forgotten = Address::repeat_byte(0x82);
    let omitted = Address::repeat_byte(0x83);
    let mut world = TestWorld::new(Fork::Osaka, "default-aware frozen layout");
    world.insert(retained, TestAccount::empty()).unwrap();
    let mut input = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    // A finite default makes it observable whether a layout-only coordinate
    // reads the old input fallback or the newly widened output fallback.
    input.balance_default = word(2);
    input.balances.remove(&retained);
    input.begin_creation(forgotten);
    input.selfdestruct(forgotten, forgotten, domain.borrowed());
    input.write_balance(forgotten, word(7));
    input.write(retained, &word(1), &word(7), domain.borrowed());
    input.write_transient(retained, &word(1), &word(7), domain.borrowed());

    let mut layout = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    layout.begin_creation(retained);
    layout.selfdestruct(retained, retained, domain.borrowed());
    layout.write_balance(retained, word(99));
    layout.write(retained, &word(2), &word(99), domain.borrowed());
    layout.write_transient(retained, &word(2), &word(99), domain.borrowed());
    let layout = layout.summary_projection(domain.borrowed()).unwrap();
    let input = input.summary_projection(domain.borrowed()).unwrap();
    let projected = input.project_to_layout(&layout, domain.borrowed());
    assert!(projected.covers(&input, domain.borrowed()));
    assert_eq!(
        projected.project_to_layout(&layout, domain.borrowed()),
        projected
    );
    let replay = projected.instantiate(domain.borrowed()).unwrap();
    assert_eq!(
        replay.read_balance(retained).singleton(),
        Some(U256::from(2))
    );
    for value in [
        replay.balance_default().clone(),
        replay.read_balance(forgotten),
    ] {
        assert_eq!(
            value.constants(),
            Some(&BTreeSet::from([U256::from(2), U256::from(7)]))
        );
    }
    assert_eq!(flags(&replay, retained), [Some(false); 3]);
    assert_eq!(flags(&replay, forgotten), [None; 3]);
    assert_eq!(flags(&replay, omitted), [None; 3]);
    for value in [
        replay.read(retained, &word(2), domain.borrowed()),
        replay.read_transient(retained, &word(2), domain.borrowed()),
    ] {
        assert_eq!(value.singleton(), Some(U256::ZERO));
    }
    assert!(
        replay
            .read(retained, &word(1), domain.borrowed())
            .numeric()
            .is_top()
    );
    assert!(
        replay
            .read_transient(retained, &word(1), domain.borrowed())
            .numeric()
            .is_top()
    );
    assert!(
        replay
            .read(omitted, &word(2), domain.borrowed())
            .numeric()
            .is_top()
    );
    assert_eq!(
        replay
            .read_transient(omitted, &word(2), domain.borrowed())
            .singleton(),
        Some(U256::ZERO)
    );
}
