//! Exact RPC words feed the ordinary native session fact protocol.
//!
//! Acquisition retains raw observations so this bridge never queries or clones
//! an opaque numeric element. Session refinement funds validation against the
//! actual immutable carrier and publishes each fact revision atomically.

use super::session::AccountObservation;
use crate::{
    Address,
    analysis::{
        AnalysisEngine, AnalysisInput, ContextPolicy, ExecutionConfig, PartitionPolicy,
        StatePolicy,
        control::Control,
        cooperation::Composition,
        layers::{AnalysisSession, CodeObservation, Observation, SessionError},
    },
    domain::EvmNumericAlgebra,
    world::{Code, EvmEntry, EvmWorld, Existence, SnapshotIdentity},
};
use std::collections::BTreeMap;

pub(super) type AcquisitionSessionResult<'a, D, C, P, S, X> = Result<
    AnalysisSession<'a, D, C, P, S, X>,
    SessionError<<X as Composition<<D as EvmNumericAlgebra>::Element>>::Error>,
>;

pub(super) fn start<'a, D, C, P, S, X>(
    world: EvmWorld<D::Element>,
    observations: &BTreeMap<Address, AccountObservation>,
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    control: &Control,
) -> AcquisitionSessionResult<'a, D, C, P, S, X>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    // Only the pinned header/source identity is moved into the native input.
    // All account constraints enter through the same paid exact-fact protocol.
    if !matches!(world.identity(), SnapshotIdentity::Chain { .. }) || !world.accounts().is_empty() {
        return Err(SessionError::Seed);
    }
    let input = AnalysisInput::new(world, entry)?;
    let mut session = AnalysisSession::new(engine, input, config)?;
    install(
        observations,
        &mut session,
        observations.keys().copied(),
        control,
    )?;
    Ok(session)
}

pub(super) fn refine<D, C, P, S, X>(
    world: &EvmWorld<D::Element>,
    observations: &BTreeMap<Address, AccountObservation>,
    session: &mut AnalysisSession<'_, D, C, P, S, X>,
    addresses: impl IntoIterator<Item = Address>,
    control: &Control,
) -> Result<(), SessionError<<X as Composition<D::Element>>::Error>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    if world.fork() != session.catalog().fork()
        || world.identity() != session.facts().snapshot().identity()
    {
        return Err(SessionError::Seed);
    }
    install(observations, session, addresses, control)
}

fn install<D, C, P, S, X>(
    acquired: &BTreeMap<Address, AccountObservation>,
    session: &mut AnalysisSession<'_, D, C, P, S, X>,
    addresses: impl IntoIterator<Item = Address>,
    control: &Control,
) -> Result<(), SessionError<<X as Composition<D::Element>>::Error>>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    control.checkpoint()?;
    let mut observations = Vec::new();
    for address in addresses {
        control.checkpoint()?;
        let Some(account) = acquired.get(&address) else {
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
        observations.push(Observation::Balance {
            address,
            value: account.balance,
        });
        observations.push(Observation::Nonce {
            address,
            value: account.nonce,
        });
        if account.existence != Existence::Unknown {
            observations.push(Observation::Existence {
                address,
                exists: account.existence == Existence::Present,
            });
        }
        observations.extend(
            account
                .storage
                .iter()
                .map(|(slot, value)| Observation::Storage {
                    address,
                    slot: *slot,
                    value: *value,
                }),
        );
    }
    let snapshot = session.facts().snapshot().clone();
    session.refine(&snapshot, observations, control)?;
    Ok(())
}
