//! 缺失代码与 storage 观测在同一固定 RPC 快照下触发重跑。
//!
//! Initial account observations never overwrite transaction effects. Refinement
//! rebuilds the graph, Store, rollback checkpoints and summary namespace from the
//! transaction entry, while retaining the same execution and acquisition limits.

mod hybrid;
pub use hybrid::{
    RpcBaselineRelation, RpcFactRestriction, RpcHybridAnalysis, RpcHybridCoverage,
    RpcPrecisionOutcome, analyze_rpc_hybrid, analyze_rpc_hybrid_with_publisher,
};

use super::{
    AnalysisEngine, ConfigError, ContextPolicy, EvmMachineFrontier, EvmWorldAnalysis,
    ExecutionConfig, FrontierReason, PartitionPolicy, StatePolicy, Status,
    control::{Cancelled, Control},
    cooperation::Composition,
    layers::SessionError,
    progress::Phase,
    transfer::StorageReadPolicy,
};
use crate::{
    Address, U256,
    world::{
        EvmEntry,
        rpc::{AccountRequest, RpcError, RpcInput, Session},
    },
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Cumulative discovery evidence; the graph belongs only to the last fixed snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct RpcAcquisition {
    /// Number of analysis rounds, including interrupted final rounds.
    pub rounds: usize,
    /// Newly discovered accounts whose complete observations were acquired.
    pub fetched_accounts: Vec<Address>,
    /// Accounts whose attempted incremental acquisition failed; no automatic retries.
    pub failed_accounts: Vec<Address>,
    /// Newly acquired initial storage observations; zero is also an observation.
    pub fetched_storage: Vec<RpcStorageSlot>,
    /// Initial slot observations whose atomic acquisition batch failed.
    pub failed_storage: Vec<RpcStorageSlot>,
    /// Typed failures remain available even if a later execution budget stops before the call.
    pub failures: Vec<RpcAccountFailure>,
    /// HTTP requests attempted across initial and incremental acquisition.
    pub requests: usize,
    /// State allocations across all rounds, including discarded graphs.
    pub states_created: usize,
}

/// A failed account or slot observation with its fixed-snapshot request evidence.
#[derive(Clone, Debug, Serialize)]
pub struct RpcAccountFailure {
    /// Account whose complete initial observation could not be acquired.
    pub address: Address,
    /// Absent for account acquisition; present for an initial storage request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<U256>,
    /// Concrete failure category, method and chain/block identity.
    pub failure: crate::world::rpc::Failure,
}

/// One initial slot at the pinned snapshot, owned independently of code address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RpcStorageSlot {
    /// Account whose transaction storage is being read.
    pub address: Address,
    /// Complete concrete slot key, including hashed mapping or array keys.
    pub slot: U256,
}

/// An acquired graph with concrete late RPC errors available to library callers.
#[derive(Debug)]
pub struct RpcAnalysis<N: crate::domain::WordElement> {
    analysis: EvmWorldAnalysis<N>,
    failures: Vec<RpcError>,
}

type RpcAnalysisResult<N, E> = Result<RpcAnalysis<N>, RpcAnalysisError<E>>;

impl<N: crate::domain::WordElement> RpcAnalysis<N> {
    /// Last graph, with typed incomplete frontiers for acquisition failures.
    pub fn analysis(&self) -> &EvmWorldAnalysis<N> {
        &self.analysis
    }

    /// Original concrete failures, retaining transport and protocol details.
    pub fn failures(&self) -> &[RpcError] {
        &self.failures
    }

    /// Consume the acquisition wrapper; serialized frontier evidence remains in the graph.
    pub fn into_analysis(self) -> EvmWorldAnalysis<N> {
        self.analysis
    }
}

/// Invalid configuration or failure acquiring the initial fixed world.
#[derive(Debug, Error)]
pub enum RpcAnalysisError<E> {
    /// Typed immutable observation/catalogue projection failed before execution.
    #[error("{0}")]
    Session(SessionError<E>),
    /// Analysis did not start because its parameters were invalid.
    #[error("{0}")]
    Config(ConfigError),
    /// The caller cancelled execution or acquisition.
    #[error("{0}")]
    Cancelled(Cancelled),
    /// No valid initial RPC world could be established.
    #[error("{0}")]
    Rpc(RpcError),
}

impl<E> From<SessionError<E>> for RpcAnalysisError<E> {
    fn from(error: SessionError<E>) -> Self {
        Self::Session(error)
    }
}

impl<E> From<Cancelled> for RpcAnalysisError<E> {
    fn from(error: Cancelled) -> Self {
        Self::Cancelled(error)
    }
}

impl<E> From<ConfigError> for RpcAnalysisError<E> {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl<E> From<RpcError> for RpcAnalysisError<E> {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

/// Analyze a fixed RPC snapshot, acquiring missing callees and finite storage reads.
///
/// The entry is acquired automatically. CALL-family and [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) resolution
/// identify missing account code; SLOAD requests missing initial values for
/// concrete storage owners and finite keys. Unknown targets/keys are not guessed,
/// and current transaction effects are never overwritten by RPC observations.
/// Each successful refinement starts a fresh graph from
/// the entry under cumulative work, transfer and state-allocation budgets.
///
/// Initial acquisition errors return `Err`. Late failures preserve an
/// `Incomplete` graph and original errors in [`RpcAnalysis::failures`].
pub fn analyze_rpc<'a, D, C, P, S, X>(
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    input: &RpcInput,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    control: &Control,
) -> RpcAnalysisResult<D::Element, <X as Composition<D::Element>>::Error>
where
    D: crate::domain::EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    control.scope(|| analyze_controlled(engine, input, entry, config, control))
}

fn analyze_controlled<'a, D, C, P, S, X>(
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    input: &RpcInput,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    control: &Control,
) -> RpcAnalysisResult<D::Element, <X as Composition<D::Element>>::Error>
where
    D: crate::domain::EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    control.checkpoint()?;
    control.observer().phase(Phase::Validating);
    config.validate()?;
    engine
        .algebra
        .spec()
        .validate()
        .map_err(ConfigError::from)?;
    entry
        .environment
        .validate_structure()
        .map_err(ConfigError::from)?;
    if let Some(observed) = entry.environment.to.as_concrete()
        && observed != entry.address
    {
        return Err(
            ConfigError::from(crate::world::environment::EnvironmentError::Destination {
                expected: entry.address,
                observed,
            })
            .into(),
        );
    }
    let mut input = input.clone();
    if !input
        .accounts
        .iter()
        .any(|account| account.address == entry.address)
    {
        input.accounts.push(AccountRequest {
            address: entry.address,
            slots: BTreeSet::new(),
        });
    }
    let initial = Session::load_with_control(&input, control);
    control.checkpoint()?;
    let mut session = initial?;
    let mut layered = session.layered(engine, entry, config.clone(), control)?;
    let mut discovery = Discovery::default();
    let mut analysis = layered.run_uncached(control, StorageReadPolicy::Discover)?;
    loop {
        control.checkpoint()?;
        control.observer().phase(Phase::Analyzing);
        discovery.rounds += 1;
        session.discovery_round(discovery.rounds);
        let mut accounts = BTreeSet::new();
        let mut storage: BTreeMap<Address, BTreeSet<U256>> = BTreeMap::new();
        for frontier in &analysis.frontiers {
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
        if accounts.is_empty() && storage.is_empty() {
            break;
        }
        let mut progress = false;
        for address in accounts {
            if let Some(reason) = layered.reserve_acquisition(1) {
                resource_frontier(&mut analysis, reason);
                break;
            }
            let acquired = session.fetch_account(address);
            control.checkpoint()?;
            match acquired {
                Ok(installed) => {
                    progress |= installed;
                    if installed {
                        session.refine_layered(&mut layered, [address], control)?;
                        discovery.fetched_accounts.insert(address);
                    }
                }
                Err(error) => {
                    discovery.account_failures.insert(address, error);
                }
            }
        }
        for (address, slots) in storage {
            // Acquisition retains raw U256 observations. Fund their fixed-width
            // bookkeeping here; the session quotes its actual N literal factory
            // at the later paid lift, without manufacturing a value to price it.
            let work = slots
                .len()
                .saturating_mul(128)
                .saturating_add(session.world().work_size());
            if let Some(reason) = layered.reserve_acquisition(work) {
                resource_frontier(&mut analysis, reason);
                break;
            }
            let acquired = session.fetch_storage(address, &slots);
            control.checkpoint()?;
            match acquired {
                Ok(installed) => {
                    if !installed.is_empty() {
                        session.refine_layered(&mut layered, [address], control)?;
                    }
                    progress |= !installed.is_empty();
                    discovery.fetched_storage.extend(
                        installed
                            .into_iter()
                            .map(|slot| RpcStorageSlot { address, slot }),
                    );
                }
                Err(error) => {
                    // 一批任一采集/身份检查失败都未安装；各键保留同一批失败证据。
                    let mut failure = error.failure();
                    failure.context.account.get_or_insert(address);
                    for slot in slots {
                        let mut evidence = failure.clone();
                        // 身份检查本身没有slot参数；记录它所属的待采集观测。
                        evidence.context.slot.get_or_insert(slot);
                        discovery
                            .storage_failures
                            .insert(RpcStorageSlot { address, slot }, evidence);
                    }
                    discovery.storage_errors.push(error);
                }
            }
        }
        if !progress {
            break;
        }
        match layered.run_uncached(control, StorageReadPolicy::Discover) {
            Ok(next) => analysis = next,
            Err(SessionError::Work) => {
                resource_frontier(&mut analysis, FrontierReason::Work);
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    control.checkpoint()?;
    control.observer().phase(Phase::Analyzing);
    analysis.work = layered.work_used();
    let acquisition = discovery.acquisition(session.requests(), layered.states_created());
    Ok(finish(analysis, acquisition, discovery))
}

#[derive(Default)]
struct Discovery {
    rounds: usize,
    fetched_accounts: BTreeSet<Address>,
    account_failures: BTreeMap<Address, RpcError>,
    fetched_storage: BTreeSet<RpcStorageSlot>,
    storage_failures: BTreeMap<RpcStorageSlot, crate::world::rpc::Failure>,
    storage_errors: Vec<RpcError>,
}

impl Discovery {
    fn acquisition(&self, requests: usize, states_created: usize) -> RpcAcquisition {
        let failures = self
            .account_failures
            .iter()
            .map(|(address, error)| RpcAccountFailure {
                address: *address,
                slot: None,
                failure: error.failure(),
            })
            .chain(
                self.storage_failures
                    .iter()
                    .map(|(key, failure)| RpcAccountFailure {
                        address: key.address,
                        slot: Some(key.slot),
                        failure: failure.clone(),
                    }),
            )
            .collect();
        RpcAcquisition {
            rounds: self.rounds,
            fetched_accounts: self.fetched_accounts.iter().copied().collect(),
            failed_accounts: self.account_failures.keys().copied().collect(),
            fetched_storage: self.fetched_storage.iter().copied().collect(),
            failed_storage: self.storage_failures.keys().copied().collect(),
            failures,
            requests,
            states_created,
        }
    }
}

fn resource_frontier<N: crate::domain::WordElement>(
    analysis: &mut EvmWorldAnalysis<N>,
    reason: FrontierReason,
) {
    analysis.frontiers.push(EvmMachineFrontier {
        from: None,
        target: None,
        pc: None,
        reason,
        range: None,
        payload: None,
    });
}

fn finish<N: crate::domain::WordElement>(
    mut analysis: EvmWorldAnalysis<N>,
    acquisition: RpcAcquisition,
    mut discovery: Discovery,
) -> RpcAnalysis<N> {
    // A previous acquisition failure remains authoritative even if a later
    // execution round stops before revisiting its demand. Disposal was prepaid
    // by root preparation; provider failures never publish terminal coverage.
    if !discovery.account_failures.is_empty() || !discovery.storage_failures.is_empty() {
        analysis.root_terminal_coverage = None;
    }
    for frontier in &mut analysis.frontiers {
        let failure = match frontier.reason {
            FrontierReason::MissingCode(address) => discovery
                .account_failures
                .get(&address)
                .map(|error| (address, error.failure())),
            FrontierReason::MissingStorage { address, slot } => discovery
                .storage_failures
                .get(&RpcStorageSlot { address, slot })
                .map(|failure| (address, failure.clone())),
            _ => None,
        };
        if let Some((address, failure)) = failure {
            frontier.reason = FrontierReason::RpcAcquisition {
                address,
                failure: Box::new(failure),
            };
        }
    }
    analysis.status = if analysis.frontiers.is_empty() {
        Status::Converged
    } else {
        Status::Incomplete
    };
    analysis.rpc_acquisition = Some(acquisition);
    let mut failures: Vec<_> = discovery.account_failures.into_values().collect();
    failures.append(&mut discovery.storage_errors);
    RpcAnalysis { analysis, failures }
}
