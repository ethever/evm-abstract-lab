//! Shared concrete collection; unknown code refinement stays fixture-specific.
use evm_abstract::{
    domain::NumericValue,
    world::{EvmEntry, EvmWorld},
};
#[path = "../support/revm.rs"]
mod concrete;
pub(super) fn execute(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
) -> concrete::Observation {
    execute_with_gas(world, entry, 10_000_000)
}
pub(super) fn execute_with_gas(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    gas: u64,
) -> concrete::Observation {
    concrete::execute_gas(world, entry, gas)
}

pub(super) fn concrete_unknown(
    world: &EvmWorld<NumericValue>,
    owner: u64,
    code: &super::support::Fixture,
) -> EvmWorld<NumericValue> {
    // World observations are immutable. Build an independent refinement rather
    // than replacing an existing Unknown observation through World::insert.
    let mut concrete = EvmWorld::<NumericValue>::new(world.fork(), world.provenance());
    for (address, account) in world.accounts() {
        if *address != super::support::address(owner) {
            concrete.insert(*address, account.clone()).unwrap();
        }
    }
    let mut account = evm_abstract::world::EvmAccount::<NumericValue>::from_hex(
        &alloy_primitives::hex::encode(&code.bytes),
        world.fork(),
    )
    .unwrap();
    account.balance = super::support::word(1_000_000);
    if let Some(observed) = world.account(super::support::address(owner)) {
        if let Some(balance) = observed.balance.singleton() {
            account.balance = evm_abstract::domain::EvmValue::<NumericValue>::constant(balance);
        }
        if let Some(nonce) = observed.nonce.singleton() {
            account.nonce = evm_abstract::domain::EvmValue::<NumericValue>::constant(nonce);
        }
        account.storage.clone_from(&observed.storage);
    }
    concrete
        .insert(super::support::address(owner), account)
        .unwrap();
    concrete
}
