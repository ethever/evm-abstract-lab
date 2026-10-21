//! Native input copies are quoted before sessions or terminal anchors own them.

use crate::{
    analysis::AnalysisInput,
    domain::WordElement,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{finish_quote, pay, value_work},
    },
    world::EvmEnvironmentValues,
};

/// Fund the container census, then return the cost of one actual environment clone.
pub(in crate::analysis) fn environment_clone<N: WordElement>(
    env: &EvmEnvironmentValues<N>,
    budget: &mut WorkBudget,
) -> Result<usize, CloneError> {
    pay(
        budget,
        env.block_hashes
            .len()
            .saturating_add(env.blob_hashes.hashes.len())
            .saturating_add(16),
    )?;
    let mut work = env
        .calldata
        .clone_work_with(budget)?
        .saturating_add(
            env.block_hashes
                .len()
                .saturating_add(env.blob_hashes.hashes.len())
                .saturating_mul(64),
        )
        .saturating_add(64);
    for value in [
        &env.value,
        &env.gas_price,
        &env.timestamp,
        &env.number,
        &env.prevrandao,
        &env.gas_limit,
        &env.base_fee,
        &env.blob_base_fee,
        &env.blob_hashes.length,
    ]
    .into_iter()
    .chain(env.chain_id.as_ref())
    {
        work = work.saturating_add(value_work(value, budget)?);
    }
    finish_quote(work, budget)
}

/// Projection may copy the base twice and intersect facts in a detached account.
/// Each native clone and membership/absence query receives its own paid envelope.
pub(super) fn pay_input<N: WordElement>(
    input: &AnalysisInput<N>,
    budget: &mut WorkBudget,
) -> Result<(), CloneError> {
    let mut work = environment_clone(&input.entry.environment, budget)?;
    pay(budget, input.world.accounts().len().saturating_add(1))?;
    for account in input.world.accounts().values() {
        pay(budget, account.storage.len().saturating_add(4))?;
        for value in account
            .storage
            .values()
            .chain([&account.balance, &account.nonce])
        {
            work = work
                .saturating_add(value_work(value, budget)?.saturating_mul(6))
                .saturating_add(value.work_size().saturating_mul(6))
                .saturating_add(value.singleton_work());
        }
        if let crate::world::Code::Runtime(program) = &account.code {
            work = work.saturating_add(program.byte_len().saturating_mul(4));
        }
    }
    let facts = input.facts.observation_count();
    work = work.saturating_add(facts.saturating_mul(128));
    pay(budget, work)
}
