//! Owner partition, native binding and concrete EVM coverage checkpoints.

use super::{
    CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, PreparedProcessStore, ProcessOwner,
};
use crate::{
    Fork,
    domain::{EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
    world::{EvmAccount, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, B256, U256};
use std::sync::Arc;

mod catalogue;
mod coverage;
mod roles;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::constant(U256::from(value))
}

fn domain() -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::default())
}

fn budget() -> WorkBudget {
    WorkBudget::new(200_000_000)
}

fn catalogue() -> Arc<OwnerCatalogue> {
    OwnerCatalogue::freeze(
        &[address(0x100), address(0x200)],
        &[CapturedProcessCode {
            source: address(0x200),
            hash: B256::repeat_byte(0x55),
            fork: Fork::Osaka,
        }],
        OwnerCatalogueLimits { names: 2, codes: 1 },
        &mut budget(),
    )
    .unwrap()
}

fn store(domain: &ValueAlgebra<ProductAlgebra>) -> EvmStore<NumericValue> {
    let mut world = EvmWorld::new(Fork::Osaka, "process-owner-preparation");
    for (owner, value) in [(0x100, 11), (0x200, 22), (0xaaa, 33), (0xbbb, 44)] {
        let mut account = EvmAccount::from_hex("00", Fork::Osaka).unwrap();
        account.storage.insert(U256::ZERO, word(value));
        account.balance = word(value + 100);
        world.insert(address(owner), account).unwrap();
    }
    EvmStore::new(&world, domain.borrowed())
}

fn prepare(
    store: &EvmStore<NumericValue>,
    catalogue: &Arc<OwnerCatalogue>,
    owner: ProcessOwner,
    domain: &ValueAlgebra<ProductAlgebra>,
) -> PreparedProcessStore<NumericValue> {
    PreparedProcessStore::prepare(store, catalogue, owner, domain.borrowed(), &mut budget())
        .unwrap()
}
