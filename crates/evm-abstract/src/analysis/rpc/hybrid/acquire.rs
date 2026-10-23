//! Genuine discovery uses the existing atomic loader and fact projection bridge.

use super::{RpcFactRestriction, RpcPrecisionOutcome};
use crate::{
    Address, U256,
    analysis::{
        ContextPolicy, EvmWorldAnalysis, FrontierReason, PartitionPolicy, StatePolicy,
        control::Control,
        cooperation::Composition,
        layers::{AnalysisSession, SessionError},
        rpc::{Discovery, RpcStorageSlot},
    },
    domain::EvmNumericAlgebra,
    world::rpc::Session,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) enum Acquired {
    Progress(bool),
    Stopped(RpcPrecisionOutcome),
}

pub(super) fn outcome(reason: &FrontierReason) -> RpcPrecisionOutcome {
    match reason {
        FrontierReason::Work | FrontierReason::SummaryWork => RpcPrecisionOutcome::Work,
        FrontierReason::Budget(limit) => RpcPrecisionOutcome::Budget(*limit),
        _ => RpcPrecisionOutcome::Incomplete,
    }
}

pub(super) fn acquire<D, C, P, S, X>(
    session: &mut Session<D::Element>,
    layered: &mut AnalysisSession<'_, D, C, P, S, X>,
    discovery: &mut Discovery,
    restrictions: &mut Vec<RpcFactRestriction>,
    report: &EvmWorldAnalysis<D::Element>,
    control: &Control,
) -> Result<Acquired, SessionError<<X as Composition<D::Element>>::Error>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    let mut accounts = BTreeSet::new();
    let mut storage: BTreeMap<Address, BTreeSet<U256>> = BTreeMap::new();
    for frontier in report.frontiers() {
        match frontier.reason {
            FrontierReason::MissingCode(address)
                if session.world().account(address).is_none()
                    && !discovery.account_failures.contains_key(&address) =>
            {
                accounts.insert(address);
            }
            FrontierReason::MissingStorage { address, slot }
                if !discovery
                    .storage_failures
                    .contains_key(&RpcStorageSlot { address, slot }) =>
            {
                storage.entry(address).or_default().insert(slot);
            }
            _ => {}
        }
    }
    let mut progress = false;
    for address in accounts {
        control.checkpoint()?;
        // Fixed-width account/revision receipt bookkeeping, on the same payer.
        if let Some(reason) = layered.reserve_acquisition(128) {
            return Ok(Acquired::Stopped(outcome(&reason)));
        }
        match session.fetch_account(address) {
            Ok(true) => {
                discovery.fetched_accounts.insert(address);
                refine(session, layered, restrictions, address, control)?;
                progress = true;
            }
            Ok(false) => {}
            Err(error) => {
                discovery.account_failures.insert(address, error);
            }
        }
        control.checkpoint()?;
    }
    for (address, slots) in storage {
        control.checkpoint()?;
        let work = slots
            .len()
            .saturating_mul(128)
            .saturating_add(session.world().work_size());
        if let Some(reason) = layered.reserve_acquisition(work) {
            return Ok(Acquired::Stopped(outcome(&reason)));
        }
        match session.fetch_storage(address, &slots) {
            Ok(installed) if !installed.is_empty() => {
                discovery.fetched_storage.extend(
                    installed
                        .into_iter()
                        .map(|slot| RpcStorageSlot { address, slot }),
                );
                refine(session, layered, restrictions, address, control)?;
                progress = true;
            }
            Ok(_) => {}
            Err(error) => {
                let mut failure = error.failure();
                failure.context.account.get_or_insert(address);
                for slot in slots {
                    let mut evidence = failure.clone();
                    evidence.context.slot.get_or_insert(slot);
                    discovery
                        .storage_failures
                        .insert(RpcStorageSlot { address, slot }, evidence);
                }
                discovery.storage_errors.push(error);
            }
        }
        control.checkpoint()?;
    }
    Ok(Acquired::Progress(progress))
}

fn refine<D, C, P, S, X>(
    session: &Session<D::Element>,
    layered: &mut AnalysisSession<'_, D, C, P, S, X>,
    restrictions: &mut Vec<RpcFactRestriction>,
    address: Address,
    control: &Control,
) -> Result<(), SessionError<<X as Composition<D::Element>>::Error>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    let previous = layered.facts().revision();
    let count = layered.observation_count();
    // This bridge checks snapshot/fork and uses StateFacts::refine plus actual
    // carrier validation. It cannot change an old observation, the base or entry.
    // Record the restriction only AFTER the atomic fact publication succeeds.
    session.refine_layered(layered, [address], control)?;
    let current = layered.facts().revision();
    if current != previous {
        restrictions.push(RpcFactRestriction {
            previous,
            current,
            added_observations: layered.observation_count().saturating_sub(count),
        });
    }
    Ok(())
}
