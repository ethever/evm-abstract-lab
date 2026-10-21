//! The same world protocol retains the actual product or finite carrier.

use super::{EvmAccount, EvmByteArray, EvmStore, EvmWorld, RangeError};
use crate::{
    Address, Fork, U256,
    domain::{EvmNumericAlgebra, EvmValue, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra},
};

fn exercise<D: EvmNumericAlgebra>(numeric: D) {
    let algebra = ValueAlgebra::new(numeric);
    let domain = algebra.borrowed();
    let word = |value: u64| EvmValue::<D::Element>::constant(U256::from(value));

    let source = EvmByteArray::<D::Element>::exact(&[0x11, 0x22, 0x33]);
    let mut memory = EvmByteArray::<D::Element>::memory();
    memory.write_byte(&word(4), &word(0x77), domain).unwrap();
    memory
        .copy_return_data(&word(0), &source, &word(50), domain)
        .unwrap();
    assert_eq!(memory.len().singleton(), Some(U256::from(64)));
    for (index, expected) in [(0, 0x11), (1, 0x22), (2, 0x33), (3, 0), (4, 0x77)] {
        assert_eq!(
            memory.byte_at(index, domain).singleton(),
            Some(U256::from(expected))
        );
    }
    let slice = memory.slice(&word(1), &word(4), domain).unwrap();
    assert_eq!(
        slice.exact_bytes_bounded(4, domain),
        Some(vec![0x22, 0x33, 0, 0x77])
    );
    assert_eq!(slice.exact_bytes_bounded(3, domain), None);

    let before = memory.clone();
    let overflow = EvmValue::<D::Element>::constant(U256::MAX);
    assert!(matches!(
        memory.expand(&overflow, &word(1), domain),
        Err(RangeError::OffsetTooLarge(_))
    ));
    assert_eq!(memory, before);
    memory.expand(&overflow, &word(0), domain).unwrap();
    assert_eq!(memory, before);

    let mut huge = EvmByteArray::<D::Element>::memory();
    let offset = U256::ONE << 192;
    huge.expand(&EvmValue::constant(offset), &word(1), domain)
        .unwrap();
    assert_eq!(huge.len().singleton(), Some(offset + U256::from(32)));
    assert!(huge.stored_bytes().next().is_none());
    assert_eq!(huge.exact_bytes_bounded(64, domain), None);

    let owner = Address::repeat_byte(0x41);
    let mut world = EvmWorld::<D::Element>::new(Fork::Osaka, "native numeric world");
    world.insert(owner, EvmAccount::empty()).unwrap();
    let fingerprint = world.fingerprint().unwrap();
    assert_eq!(fingerprint, world.clone().fingerprint().unwrap());
    let mut store = EvmStore::new_checked(&world, domain).unwrap();
    let saved = store.snapshot();
    store
        .write_checked(owner, &word(1), &word(7), domain)
        .unwrap();
    assert_eq!(
        store
            .read_checked(owner, &word(1), domain)
            .unwrap()
            .singleton(),
        Some(U256::from(7))
    );
    let summary = store.summary_projection(domain).unwrap();
    let rebuilt = summary.instantiate(domain).unwrap();
    assert_eq!(
        rebuilt
            .read_checked(owner, &word(1), domain)
            .unwrap()
            .singleton(),
        Some(U256::from(7))
    );
    store.restore(saved);
    assert_eq!(
        store
            .read_checked(owner, &word(1), domain)
            .unwrap()
            .singleton(),
        Some(U256::ZERO)
    );

    let possible = domain.join(&word(1), &word(3));
    assert!(possible.contains(U256::ONE));
    assert!(possible.contains(U256::from(3)));
    let native = serde_json::to_value(&possible).unwrap();
    assert!(native.is_object());
    assert!(native.get("numeric").unwrap().is_object());
}

#[test]
fn product_world_memory_and_rollback_use_native_algebra() {
    exercise(ProductAlgebra::default());
}

#[test]
fn finite_world_memory_and_rollback_use_native_carrier() {
    exercise(FiniteSetAlgebra::default());
}
