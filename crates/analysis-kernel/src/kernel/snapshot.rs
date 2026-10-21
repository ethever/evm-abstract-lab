//! Unpublished replay metadata with checked, atomic installation.

use super::{Job, Kernel, Meter, reserve};
use crate::{Epoch, KernelError};
use std::sync::Arc;

/// Detached graph metadata for speculative replay.
///
/// Semantic stores must be copied separately after their own work reservation.
/// Copy sidecar handles only into those detached stores; never mutate live values
/// through a fork. All work uses the original root meter, including failed work.
/// Aborting refunds nothing. New handles from an aborted fork cannot alias live
/// handles even if a later allocation reuses the same vector index.
pub struct KernelFork<J> {
    base_epoch: Epoch,
    base_allocation: Epoch,
    base_revision: u64,
    staged: Kernel<J>,
}
impl<J> KernelFork<J> {
    /// Mutate the unpublished metadata, using detached semantic stores only.
    pub fn kernel(&mut self) -> &mut Kernel<J> {
        &mut self.staged
    }
    /// Inspect the unpublished graph without borrowing it mutably.
    pub fn view(&self) -> &Kernel<J> {
        &self.staged
    }
}

impl<J> Kernel<J> {
    /// Reserve metadata copying, then fork without cloning application tasks.
    ///
    /// Existing task values are immutable Arc handles. Each fork uses a fresh
    /// allocation namespace while preserving the existing handles' identities.
    /// Existing suspended tokens work in staged copies; commit invalidates a
    /// stale live snapshot whenever any live graph transition has occurred.
    pub fn fork<M: Meter>(&self, meter: &mut M) -> Result<KernelFork<J>, KernelError<M::Stop>> {
        if self.revision == u64::MAX {
            return Err(KernelError::VersionExhausted);
        }
        // Pay the outer scan before walking per-fact and per-job containers.
        reserve(
            meter,
            self.facts
                .len()
                .saturating_add(self.jobs.len())
                .saturating_add(self.scopes.len())
                .saturating_add(1),
        )?;
        let work = self
            .facts
            .iter()
            .fold(0usize, |n, f| {
                n.saturating_add(f.readers.len())
                    .saturating_add(f.providers.len())
                    .saturating_add(f.demands.len())
                    .saturating_add(f.assumptions.len())
                    .saturating_add(f.coverage.len().saturating_mul(2))
            })
            .saturating_add(self.jobs.iter().fold(0usize, |n, j| {
                n.saturating_add(j.reads.len())
                    .saturating_add(j.required.len())
                    .saturating_add(j.provides.len())
            }))
            .saturating_add(self.scopes.iter().fold(0usize, |n, s| {
                n.saturating_add(s.jobs.len())
                    .saturating_add(s.facts.len())
                    .saturating_add(s.assumptions.len())
                    .saturating_add(s.open_obligations.len())
            }))
            .saturating_add(self.scope_ids.len())
            .saturating_add(self.obligations.len())
            .saturating_add(self.worklist.work_size())
            .saturating_add(self.facts.len())
            .saturating_add(self.jobs.len().saturating_mul(2))
            .saturating_add(self.scopes.len())
            .saturating_add(1);
        reserve(meter, work)?;
        let allocation = Epoch::fresh().map_err(|_| KernelError::VersionExhausted)?;
        Ok(KernelFork {
            base_epoch: self.epoch,
            base_allocation: self.allocation,
            base_revision: self.revision,
            staged: Kernel {
                epoch: self.epoch,
                allocation,
                revision: self.revision,
                stats: self.stats,
                facts: self.facts.clone(),
                jobs: self
                    .jobs
                    .iter()
                    .map(|j| Job {
                        id: j.id,
                        scope: j.scope,
                        task: j.task.as_ref().map(Arc::clone),
                        reads: j.reads.clone(),
                        required: j.required.clone(),
                        provides: j.provides.clone(),
                        state: j.state,
                        evaluation: j.evaluation,
                        evaluation_birth: j.evaluation_birth,
                        limited: j.limited,
                    })
                    .collect(),
                scopes: self.scopes.clone(),
                scope_ids: self.scope_ids.clone(),
                obligations: self.obligations.clone(),
                worklist: self.worklist.clone(),
            },
        })
    }

    /// Commit an unpublished graph with no external semantic store to replace.
    pub fn commit<M: Meter>(
        &mut self,
        fork: KernelFork<J>,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.commit_with(fork, meter, 0, || {})
    }

    /// Reserve the last installation, then replace semantic stores and metadata.
    ///
    /// `install` is infallible and must replace only the matching detached
    /// semantic graph. No fallible operation follows it. The guarantee covers
    /// typed errors/cancellation, not panics or process allocation failure.
    pub fn commit_with<M: Meter>(
        &mut self,
        fork: KernelFork<J>,
        meter: &mut M,
        install_work: usize,
        install: impl FnOnce(),
    ) -> Result<(), KernelError<M::Stop>> {
        if fork.base_epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        if self.revision == u64::MAX
            || fork.base_revision != self.revision
            || fork.base_allocation != self.allocation
        {
            return Err(KernelError::StaleFork);
        }
        // Replacing the old metadata drops all its containers. That traversal is
        // real even when the adapter's semantic graph can be installed by swap.
        reserve(
            meter,
            self.facts
                .len()
                .saturating_add(self.jobs.len())
                .saturating_add(self.scopes.len())
                .saturating_add(1),
        )?;
        let drop_work = self
            .facts
            .iter()
            .fold(0usize, |n, f| {
                n.saturating_add(f.readers.len())
                    .saturating_add(f.providers.len())
                    .saturating_add(f.demands.len())
                    .saturating_add(f.assumptions.len())
                    .saturating_add(f.coverage.len().saturating_mul(2))
            })
            .saturating_add(self.jobs.iter().fold(0usize, |n, j| {
                n.saturating_add(j.reads.len())
                    .saturating_add(j.required.len())
                    .saturating_add(j.provides.len())
            }))
            .saturating_add(self.scopes.iter().fold(0usize, |n, s| {
                n.saturating_add(s.jobs.len())
                    .saturating_add(s.facts.len())
                    .saturating_add(s.assumptions.len())
                    .saturating_add(s.open_obligations.len())
            }))
            .saturating_add(self.scope_ids.len())
            .saturating_add(self.obligations.len())
            .saturating_add(self.worklist.work_size())
            .saturating_add(self.facts.len())
            .saturating_add(self.jobs.len())
            .saturating_add(self.scopes.len())
            .saturating_add(install_work)
            .saturating_add(1);
        reserve(meter, drop_work)?;
        install();
        *self = fork.staged;
        self.touch();
        Ok(())
    }
}
