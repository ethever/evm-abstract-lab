//! Missing code requests refine one fixed RPC snapshot between complete runs.
//!
//! Initial account observations never overwrite transaction effects. Refinement
//! rebuilds the graph, Store, rollback checkpoints and summary namespace from the
//! transaction entry, while retaining the same execution and acquisition limits.

use super::{
    ConfigError, ExecutionConfig, FrontierReason, Limit, MachineFrontier, Status, WorldAnalysis,
    engine::{self, Counters},
};
use crate::{
    Address,
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
    /// Typed failures remain available even if a later execution budget stops before the call.
    pub failures: Vec<RpcAccountFailure>,
    /// HTTP requests attempted across initial and incremental acquisition.
    pub requests: usize,
    /// State allocations across all rounds, including discarded graphs.
    pub states_created: usize,
}

/// A failed discovered account together with its fixed-snapshot request evidence.
#[derive(Clone, Debug, Serialize)]
pub struct RpcAccountFailure {
    /// Account whose complete initial observation could not be acquired.
    pub address: Address,
    /// Concrete failure category, method and chain/block identity.
    pub failure: crate::world::rpc::Failure,
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
    /// No valid initial RPC world could be established.
    #[error("{0}")]
    Rpc(RpcError),
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

/// Analyze a fixed RPC snapshot, acquiring concrete missing callees on demand.
///
/// The entry is acquired automatically. CALL-family and EIP-7702 resolution
/// identify missing account code; unknown targets are never guessed, unrequested
/// storage remains unknown, and already observed transaction code is never
/// replaced by a refetch. Each successful refinement starts a fresh graph from
/// the entry under cumulative work, transfer and state-allocation budgets.
///
/// Initial acquisition errors return `Err`. Late failures preserve an
/// `Incomplete` graph and original errors in [`RpcAnalysis::failures`].
pub fn analyze_rpc(
    input: &RpcInput,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<RpcAnalysis, RpcAnalysisError> {
    config.domain()?;
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
    let mut session = Session::load(&input)?;
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut fetched = BTreeSet::new();
    let mut failures = BTreeMap::new();
    let mut rounds = 0;
    let mut analysis;
    loop {
        rounds += 1;
        // Reserve copies before creating an owned input for the next engine run.
        let copy_work = session
            .world()
            .work_size()
            .saturating_add(entry.calldata.work_size())
            .saturating_add(entry.value.work_size());
        if !budget.charge(copy_work) {
            let acquisition = RpcAcquisition {
                rounds,
                fetched_accounts: fetched.into_iter().collect(),
                failed_accounts: failures.keys().copied().collect(),
                failures: failure_evidence(&failures),
                requests: session.requests(),
                states_created: counters.states,
            };
            // Move the cache instead of cloning after the reservation failed.
            let world = session.into_world();
            analysis = engine::run_metered(world, entry, config, &mut budget, &mut counters)?;
            return Ok(finish(analysis, acquisition, failures));
        }
        analysis = engine::run_metered(
            session.world().clone(),
            entry.clone(),
            config.clone(),
            &mut budget,
            &mut counters,
        )?;
        let missing: BTreeSet<_> = analysis
            .frontiers
            .iter()
            .filter_map(|frontier| {
                if let FrontierReason::MissingCode(address) = frontier.reason {
                    // Existing unknown code is an execution overlay, not missing initial facts.
                    (session.world().account(address).is_none() && !failures.contains_key(&address))
                        .then_some(address)
                } else {
                    None
                }
            })
            .collect();
        if missing.is_empty() {
            break;
        }
        let mut progress = false;
        for address in missing {
            // A new snapshot would need a new root state and a block transfer.
            let limit = if budget.exhausted() {
                Some(FrontierReason::Work)
            } else if counters.states >= config.analysis.max_states {
                Some(FrontierReason::Budget(Limit::States))
            } else if counters.transfers >= config.analysis.max_transfers {
                Some(FrontierReason::Budget(Limit::Transfers))
            } else if !budget.charge(1) {
                Some(FrontierReason::Work)
            } else {
                None
            };
            if let Some(reason) = limit {
                analysis.frontiers.push(MachineFrontier {
                    from: None,
                    target: None,
                    pc: None,
                    reason,
                });
                break;
            }
            match session.fetch_account(address) {
                Ok(installed) => {
                    progress |= installed;
                    if installed {
                        fetched.insert(address);
                    }
                }
                Err(error) => {
                    failures.insert(address, error);
                }
            }
        }
        if !progress {
            break;
        }
    }
    analysis.work = budget.used();
    let acquisition = RpcAcquisition {
        rounds,
        fetched_accounts: fetched.into_iter().collect(),
        failed_accounts: failures.keys().copied().collect(),
        failures: failure_evidence(&failures),
        requests: session.requests(),
        states_created: counters.states,
    };
    Ok(finish(analysis, acquisition, failures))
}

fn failure_evidence(failures: &BTreeMap<Address, RpcError>) -> Vec<RpcAccountFailure> {
    failures
        .iter()
        .map(|(address, error)| RpcAccountFailure {
            address: *address,
            failure: error.failure(),
        })
        .collect()
}

fn finish(
    mut analysis: WorldAnalysis,
    acquisition: RpcAcquisition,
    failures: BTreeMap<Address, RpcError>,
) -> RpcAnalysis {
    for frontier in &mut analysis.frontiers {
        if let FrontierReason::MissingCode(address) = frontier.reason
            && let Some(error) = failures.get(&address)
        {
            frontier.reason = FrontierReason::RpcAcquisition {
                address,
                failure: Box::new(error.failure()),
            };
        }
    }
    analysis.status = if analysis.frontiers.is_empty() {
        Status::Converged
    } else {
        Status::Incomplete
    };
    analysis.rpc_acquisition = Some(acquisition);
    RpcAnalysis {
        analysis,
        failures: failures.into_values().collect(),
    }
}
