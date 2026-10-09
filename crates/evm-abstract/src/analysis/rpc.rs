//! 缺失代码与 storage 观测在同一固定 RPC 快照下触发重跑。
//!
//! Initial account observations never overwrite transaction effects. Refinement
//! rebuilds the graph, Store, rollback checkpoints and summary namespace from the
//! transaction entry, while retaining the same execution and acquisition limits.

use super::{
    ConfigError, ExecutionConfig, FrontierReason, Limit, MachineFrontier, Status, WorldAnalysis,
    control::{Cancelled, Control},
    engine::{self, Counters},
    progress::{Observer, Phase},
    transfer::StorageReadPolicy,
};
use crate::{
    Address, U256,
    resource::WorkBudget,
    world::{
        Entry,
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
pub struct RpcAnalysis {
    analysis: WorldAnalysis,
    failures: Vec<RpcError>,
}

impl RpcAnalysis {
    /// Last graph, with typed incomplete frontiers for acquisition failures.
    pub fn analysis(&self) -> &WorldAnalysis {
        &self.analysis
    }

    /// Original concrete failures, retaining transport and protocol details.
    pub fn failures(&self) -> &[RpcError] {
        &self.failures
    }

    /// Consume the acquisition wrapper; serialized frontier evidence remains in the graph.
    pub fn into_analysis(self) -> WorldAnalysis {
        self.analysis
    }
}

/// Invalid configuration or failure acquiring the initial fixed world.
#[derive(Debug, Error)]
pub enum RpcAnalysisError {
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

impl From<Cancelled> for RpcAnalysisError {
    fn from(error: Cancelled) -> Self {
        Self::Cancelled(error)
    }
}

impl From<ConfigError> for RpcAnalysisError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<RpcError> for RpcAnalysisError {
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
pub fn analyze_rpc(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<RpcAnalysis, RpcAnalysisError> {
    analyze_rpc_with_control(input, entry, config, &Control::default())
}

/// Analyze with bounded progress reporting and caller-owned cancellation.
pub fn analyze_rpc_with_control(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
    control: &Control,
) -> Result<RpcAnalysis, RpcAnalysisError> {
    control.scope(|| analyze_controlled(input, entry, config, control))
}

/// Analyze with progress observations and no cancellation.
pub fn analyze_rpc_with_observer(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
    observer: &Observer,
) -> Result<RpcAnalysis, RpcAnalysisError> {
    analyze_rpc_with_control(
        input,
        entry,
        config,
        &Control::with_observer(observer.clone()),
    )
}

fn analyze_controlled(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
    control: &Control,
) -> Result<RpcAnalysis, RpcAnalysisError> {
    control.checkpoint()?;
    control.observer().phase(Phase::Validating);
    config.domain()?;
    entry.environment.validate().map_err(ConfigError::from)?;
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
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::with_observer(control.observer());
    let mut discovery = Discovery::default();
    let mut analysis;
    loop {
        control.checkpoint()?;
        discovery.rounds += 1;
        session.discovery_round(discovery.rounds);
        // 重跑复制也使用累计账本，不让补入的新初始事实重置预算。
        let copy_work = session
            .world()
            .work_size()
            .saturating_add(entry.environment.work_size());
        if !budget.charge(copy_work) {
            let acquisition = discovery.acquisition(session.requests(), counters.states);
            let world = session.into_world();
            analysis = engine::run_metered(
                world,
                entry,
                config,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Discover,
            )?;
            control.checkpoint()?;
            return Ok(finish(analysis, acquisition, discovery));
        }
        analysis = engine::run_metered(
            session.world().clone(),
            entry.clone(),
            config.clone(),
            &mut budget,
            &mut counters,
            StorageReadPolicy::Discover,
        )?;
        control.checkpoint()?;
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
            if let Some(reason) = refinement_limit(&config, &counters, &mut budget, 1) {
                resource_frontier(&mut analysis, reason);
                break;
            }
            let acquired = session.fetch_account(address);
            control.checkpoint()?;
            match acquired {
                Ok(installed) => {
                    progress |= installed;
                    if installed {
                        discovery.fetched_accounts.insert(address);
                    }
                }
                Err(error) => {
                    discovery.account_failures.insert(address, error);
                }
            }
        }
        for (address, slots) in storage {
            // 覆盖 key 扫描、快照初始账户复制与 slot 观测安装的工作。
            let slot_work = crate::domain::AbstractValue::constant(U256::ZERO)
                .work_size()
                .saturating_add(4);
            let work = slots
                .len()
                .saturating_mul(slot_work.saturating_mul(4))
                .saturating_add(session.world().work_size());
            if let Some(reason) = refinement_limit(&config, &counters, &mut budget, work) {
                resource_frontier(&mut analysis, reason);
                break;
            }
            let acquired = session.fetch_storage(address, &slots);
            control.checkpoint()?;
            match acquired {
                Ok(installed) => {
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
    }
    control.checkpoint()?;
    analysis.work = budget.used();
    let acquisition = discovery.acquisition(session.requests(), counters.states);
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

fn refinement_limit(
    config: &ExecutionConfig,
    counters: &Counters,
    budget: &mut WorkBudget,
    work: usize,
) -> Option<FrontierReason> {
    if budget.exhausted() {
        Some(FrontierReason::Work)
    } else if counters.states >= config.analysis.max_states {
        Some(FrontierReason::Budget(Limit::States))
    } else if counters.transfers >= config.analysis.max_transfers {
        Some(FrontierReason::Budget(Limit::Transfers))
    } else if !budget.charge(work) {
        Some(FrontierReason::Work)
    } else {
        None
    }
}

fn resource_frontier(analysis: &mut WorldAnalysis, reason: FrontierReason) {
    analysis.frontiers.push(MachineFrontier {
        from: None,
        target: None,
        pc: None,
        reason,
    });
}

fn finish(
    mut analysis: WorldAnalysis,
    acquisition: RpcAcquisition,
    mut discovery: Discovery,
) -> RpcAnalysis {
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
