//! Copy immutable original input, never a process quotient or execution Store.

use crate::{
    analysis::layers::payment::environment_clone,
    domain::WordElement,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, pay, value_work},
    },
    world::{Code, EvmEntry, EvmWorld, SnapshotIdentity},
};

pub(super) fn copy<N: WordElement>(
    world: &EvmWorld<N>,
    entry: &EvmEntry<N>,
    budget: &mut WorkBudget,
) -> Result<(EvmWorld<N>, EvmEntry<N>), CloneError> {
    // run_reusing owns world/entry before initial() forms any process input.
    // Transaction effects live in separate Stores and cannot mutate this source.
    let mut work = environment_clone(&entry.environment, budget)?;
    pay(budget, world.accounts().len().saturating_add(1))?;
    work = work
        .saturating_add(128)
        .saturating_add(world.provenance().len());
    if let SnapshotIdentity::Offline { label } = world.identity() {
        work = work.saturating_add(label.len());
    }
    for account in world.accounts().values() {
        pay(budget, account.storage.len().saturating_add(4))?;
        work = work
            .saturating_add(account.storage.len().saturating_mul(64))
            .saturating_add(64);
        for value in account
            .storage
            .values()
            .chain([&account.balance, &account.nonce])
        {
            work = work.saturating_add(value_work(value, budget)?);
        }
        if let Code::Runtime(program) = &account.code {
            // Program Clone shares decoded immutable backing; the conservative
            // byte envelope also covers the retained code handle and metadata.
            work = work
                .saturating_add(program.byte_len().saturating_mul(4))
                .saturating_add(16);
        }
    }
    pay(budget, work)?;
    let copied = (world.clone(), entry.clone());
    checkpoint(budget)?;
    Ok(copied)
}

#[cfg(test)]
mod tests;
