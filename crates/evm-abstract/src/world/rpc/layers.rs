//! RPC is an exact-observation producer for the ordinary offline session layer.
//!
//! Transport acquisition stays atomic in `rpc::Session`. This adapter compiles
//! code separately and translates only supplied facts, never account defaults.

use crate::{
    Address,
    analysis::{
        ExecutionConfig,
        layers::{
            AnalysisSession, CodeCatalog, CodeObservation, Observation, SessionError, SnapshotId,
            StateFacts,
        },
    },
    world::{Code, Entry, Existence, SnapshotIdentity, World},
};
use std::sync::Arc;

pub(super) fn start(
    world: &World,
    seed: World,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<AnalysisSession, SessionError> {
    let snapshot = match world.identity() {
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => SnapshotId::chain(*chain_id, *block_hash),
        SnapshotIdentity::Offline { .. } => return Err(SessionError::Seed),
    };
    let mut session = AnalysisSession::with_seed(
        Arc::new(CodeCatalog::new(world.fork())),
        StateFacts::new(snapshot),
        None,
        entry,
        config,
        seed,
    )?;
    refine(world, &mut session, world.accounts().keys().copied())?;
    Ok(session)
}

pub(super) fn refine(
    world: &World,
    session: &mut AnalysisSession,
    addresses: impl IntoIterator<Item = Address>,
) -> Result<(), SessionError> {
    if world.fork() != session.catalog().fork()
        || world.identity() != session.facts().snapshot().identity()
    {
        return Err(SessionError::Seed);
    }
    let mut observations = Vec::new();
    for address in addresses {
        let Some(account) = world.account(address) else {
            continue;
        };
        let code = match &account.code {
            Code::Runtime(program) if program.byte_len() == 0 => Some(CodeObservation::Empty),
            Code::Runtime(program) => Some(CodeObservation::Runtime(
                session.supply_runtime(program.clone())?,
            )),
            Code::Empty => Some(CodeObservation::Empty),
            Code::Delegation(target) => Some(CodeObservation::Delegation(*target)),
            Code::Unknown => None,
        };
        if let Some(code) = code {
            observations.push(Observation::Code { address, code });
        }
        if let Some(value) = account.balance.singleton() {
            observations.push(Observation::Balance { address, value });
        }
        if let Some(value) = account.nonce.singleton() {
            observations.push(Observation::Nonce { address, value });
        }
        if account.existence != Existence::Unknown {
            observations.push(Observation::Existence {
                address,
                exists: account.existence == Existence::Present,
            });
        }
        if !account.storage_unknown {
            observations.push(Observation::StorageComplete { address });
        }
        for (slot, value) in &account.storage {
            if let Some(value) = value.singleton() {
                observations.push(Observation::Storage {
                    address,
                    slot: *slot,
                    value,
                });
            }
        }
    }
    let snapshot = session.facts().snapshot().clone();
    session.refine(&snapshot, observations)?;
    Ok(())
}
