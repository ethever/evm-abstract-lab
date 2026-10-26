//! Retain the original certificate before entering any optional RPC precision.

use super::{
    RpcBaselineRelation, RpcHybridAnalysis, RpcHybridCoverage, RpcHybridResult,
    RpcPrecisionOutcome,
    acquire::{self, Acquired},
};
use crate::{
    analysis::{
        AnalysisEngine, ConfigError, ContextPolicy, DetailState, ExecutionConfig, FrontierReason,
        HybridConfig, PartitionPolicy, StatePolicy,
        control::{Cancelled, Control},
        cooperation::{Composition, CooperationError},
        layers::SessionError,
        progress::Phase,
        publication::{NoopPublisher, StagePublisher},
        rpc::{Discovery, RpcAnalysisError},
        transfer::StorageReadPolicy,
    },
    domain::EvmNumericAlgebra,
    world::{
        EvmEntry,
        rpc::{AccountRequest, RpcInput, Session},
    },
};
use std::collections::BTreeSet;

/// Acquire the authentic initial RPC snapshot, close a conservative baseline,
/// then spend only the retained ledger's optional allowance on native detail.
/// Concrete demands use the ordinary atomic RPC loader and root-restart protocol.
///
/// Initial failures return `Err`. Once the original baseline is retained,
/// acquisition/refinement failure and work refusal are separate typed outcomes.
/// Cancellation always returns the public typed error, including after baseline
/// closure. Optional stops cannot relabel its revision or erase its proof.
/// A partial original baseline never starts optional work or becomes certified.
pub fn analyze_rpc_hybrid<'a, D, C, P, S, X>(
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    input: &RpcInput,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    hybrid: HybridConfig,
    control: &Control,
) -> RpcHybridResult<D::Element, <X as Composition<D::Element>>::Error>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    analyze_rpc_hybrid_with_publisher(
        engine,
        input,
        entry,
        config,
        hybrid,
        control,
        &mut NoopPublisher,
    )
}

/// Publish the authentic closed original model before optional fixed-snapshot
/// discovery. Every later restriction/restart uses the same cumulative payer.
/// Retained publications keep their original revision even on cancellation.
pub fn analyze_rpc_hybrid_with_publisher<'a, D, C, P, S, X, T>(
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    input: &RpcInput,
    entry: EvmEntry<D::Element>,
    config: ExecutionConfig,
    hybrid: HybridConfig,
    control: &Control,
    publisher: &mut T,
) -> RpcHybridResult<D::Element, <X as Composition<D::Element>>::Error>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
    T: StagePublisher<D::Element>,
{
    control.scope(|| {
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
            return Err(ConfigError::from(
                crate::world::environment::EnvironmentError::Destination {
                    expected: entry.address,
                    observed,
                },
            )
            .into());
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
        let mut layered = session
            .layered(engine, entry, config, control)
            .map_err(session_error)?;
        let mut baseline = layered
            .run_hybrid_uncached(
                HybridConfig {
                    max_detail_work: 0,
                    ..hybrid
                },
                StorageReadPolicy::Abstract,
                control,
            )
            .map_err(session_error)?;
        // The initial operation deliberately omitted Native execution, while
        // the outer RPC request may still admit it. Publish the real requested
        // allowance and Pending state before any precision acquisition/copy.
        baseline.defer_detail(hybrid);
        if baseline.coverage().baseline_current {
            publisher.publish(&baseline);
            publisher.checkpoint()?;
        }
        let initial_work = layered.work_used();
        let mut latest = None;
        let mut retained_detail = None;
        let mut restrictions = Vec::new();
        let mut discovery = Discovery {
            rounds: 1,
            ..Discovery::default()
        };
        let mut preparation_failure = None;
        let mut precision = if !baseline.coverage().baseline_current {
            RpcPrecisionOutcome::BaselinePending
        } else {
            RpcPrecisionOutcome::Disabled
        };
        if baseline.coverage().baseline_current
            && hybrid.max_detail_work > 0
            && hybrid.max_detail_regions > 0
        {
            // There are no mandatory phases after this point. Permanently lower
            // the existing ledger; acquisition/projection/restarts all share it.
            if let Err(error) = layered.limit_precision_work(hybrid.max_detail_work) {
                precision = session_outcome(&error)?;
                preparation_failure = Some(error);
            }
            while preparation_failure.is_none() {
                control.checkpoint()?;
                if let Some(reason) = layered.reserve_acquisition(0) {
                    precision = acquire::outcome(&reason);
                    break;
                }
                discovery.rounds += 1;
                session.discovery_round(discovery.rounds);
                control.observer().phase(Phase::Analyzing);
                let report =
                    match layered.run_hybrid_uncached(hybrid, StorageReadPolicy::Discover, control)
                    {
                        Ok(report) => report,
                        Err(error) => {
                            precision = session_outcome(&error)?;
                            preparation_failure = Some(error);
                            break;
                        }
                    };
                if report.detail().is_some() {
                    retained_detail = None;
                } else if latest.as_ref().is_some_and(
                    |r: &crate::analysis::HybridAnalysis<D::Element>| r.detail().is_some(),
                ) {
                    // Move the last useful native graph and its OWN coverage;
                    // no clone, rehydration or new-revision currency is minted.
                    retained_detail = latest.take();
                }
                latest = Some(report);
                let report = latest.as_ref().expect("latest attempt was retained");
                if report.coverage().detail_current {
                    precision = RpcPrecisionOutcome::Current;
                    break;
                }
                if let Some(reason) = layered.reserve_acquisition(0) {
                    precision = acquire::outcome(&reason);
                    break;
                }
                let Some(detail) = report.detail() else {
                    precision = if report.coverage().detail == DetailState::Work {
                        RpcPrecisionOutcome::Work
                    } else {
                        report
                            .baseline()
                            .frontiers()
                            .iter()
                            .find(|f| {
                                matches!(
                                    f.reason,
                                    FrontierReason::Work
                                        | FrontierReason::SummaryWork
                                        | FrontierReason::Budget(_)
                                )
                            })
                            .map_or(RpcPrecisionOutcome::Incomplete, |f| {
                                acquire::outcome(&f.reason)
                            })
                    };
                    break;
                };
                match acquire::acquire(
                    &mut session,
                    &mut layered,
                    &mut discovery,
                    &mut restrictions,
                    detail,
                    control,
                ) {
                    Ok(Acquired::Progress(true)) => {}
                    Ok(Acquired::Progress(false)) => {
                        precision = if discovery.account_failures.is_empty()
                            && discovery.storage_failures.is_empty()
                        {
                            RpcPrecisionOutcome::Incomplete
                        } else {
                            RpcPrecisionOutcome::AcquisitionFailed
                        };
                        break;
                    }
                    Ok(Acquired::Stopped(reason)) => {
                        precision = reason;
                        break;
                    }
                    Err(error) => {
                        precision = session_outcome(&error)?;
                        preparation_failure = Some(error);
                        break;
                    }
                }
            }
        }
        // A retained payer can be cancelled independently of today's ambient
        // Control, including between a checkpoint and a resource guard. Do not
        // publish that stop as ordinary Work or successful RPC completion.
        control.checkpoint()?;
        if layered.payer_cancelled() {
            return Err(Cancelled.into());
        }
        let current_revision = layered.facts().revision();
        let baseline_revision = baseline.baseline().revision();
        let baseline_relation = if !baseline.coverage().baseline_current {
            RpcBaselineRelation::Uncertified
        } else if baseline_revision == current_revision {
            RpcBaselineRelation::SameRevision
        } else {
            RpcBaselineRelation::InitialFactRestriction
        };
        let detail_revision = latest
            .as_ref()
            .filter(|r| r.detail().is_some())
            .or(retained_detail.as_ref())
            .and_then(|r| r.detail())
            .map(|r| r.revision());
        let coverage = RpcHybridCoverage {
            snapshot: baseline.baseline().snapshot().clone(),
            baseline_revision,
            current_revision,
            detail_revision,
            detail_at_current_revision: detail_revision == Some(current_revision),
            baseline_relation,
            restrictions,
            precision,
            acquisition: discovery.acquisition(session.requests(), layered.states_created()),
            work: layered.work_used(),
            precision_work: layered.work_used().saturating_sub(initial_work),
            max_precision_work: hybrid.max_detail_work,
            transfers: layered.transfers_used(),
        };
        let mut failures: Vec<_> = discovery.account_failures.into_values().collect();
        failures.append(&mut discovery.storage_errors);
        Ok(RpcHybridAnalysis {
            baseline,
            latest,
            retained_detail,
            coverage,
            failures,
            preparation_failure,
        })
    })
}

fn session_outcome<E>(error: &SessionError<E>) -> Result<RpcPrecisionOutcome, Cancelled> {
    match error {
        SessionError::Work => Ok(RpcPrecisionOutcome::Work),
        SessionError::Cancelled(_) | SessionError::Cooperation(CooperationError::Cancelled) => {
            Err(Cancelled)
        }
        _ => Ok(RpcPrecisionOutcome::PreparationFailed),
    }
}

fn session_error<E>(error: SessionError<E>) -> RpcAnalysisError<E> {
    match error {
        SessionError::Cancelled(_) | SessionError::Cooperation(CooperationError::Cancelled) => {
            RpcAnalysisError::Cancelled(Cancelled)
        }
        error => RpcAnalysisError::Session(error),
    }
}
