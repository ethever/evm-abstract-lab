//! Initial storage dependencies affect execution reuse without changing Store JSON.

use evm_abstract::{
    Address, Fork, U256,
    domain::{AbstractValue, Domain},
    world::{Account, Store, World},
};

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn partial_world() -> (World, Address) {
    let address = Address::repeat_byte(0x31);
    let mut account = Account::unknown();
    account.storage.insert(U256::from(4), AbstractValue::top());
    let mut world = World::new(Fork::Osaka, "initial storage dependencies");
    world.insert(address, account).unwrap();
    (world, address)
}

#[test]
fn strong_top_write_changes_reuse_state_and_work_without_changing_json() {
    let (world, address) = partial_world();
    let initial = Store::new(&world);
    let mut written = initial.clone();
    written.write(address, &word(4), &AbstractValue::top(), Domain::default());

    assert_eq!(written.slots(), initial.slots());
    assert_eq!(
        serde_json::to_value(&written).unwrap(),
        serde_json::to_value(&initial).unwrap()
    );
    assert_ne!(
        written, initial,
        "strong writes discard initial dependencies"
    );
    assert!(written.work_size() > initial.work_size());
}

#[test]
fn a_product_singleton_is_a_strong_storage_key() {
    let (world, address) = partial_world();
    let mut store = Store::new(&world);
    let key = AbstractValue::unsigned_range(U256::from(4), U256::from(4)).unwrap();
    assert!(key.constants().is_none());
    store.write(address, &key, &word(7), Domain::default());
    assert_eq!(store.read(address, &word(4), Domain::default()), word(7));
    assert_eq!(store.read(address, &key, Domain::default()), word(7));
}
