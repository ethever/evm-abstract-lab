//! Retire graph edges and typed payloads using scope/job ownership indexes.
//!
//! Fact and job IDs stay as checked, empty tombstones. Cleanup visits the retired
//! scope's own facts/jobs and their adjacent edges, never the full fact history.
//! Typed channels group cells by scope so deep payload reclamation also visits
//! only the selected retired scope, after paying its possible last-owner drops.

use super::{JobState, Kernel, Meter, reserve, tree_work};
use crate::{Availability, Capability, Channel, KernelError, Scope, algebra::PublicationPolicy};

impl<J> Kernel<J> {
    pub(super) fn retirement_work<M: Meter>(
        &self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        let generation = &self.scopes[scope.index];
        reserve(
            meter,
            generation
                .jobs
                .len()
                .saturating_add(generation.facts.len())
                .saturating_add(generation.assumptions.len())
                .saturating_add(1),
        )?;
        // All reverse sets are bounded by retained identities; only logarithmic
        // bounds consult epoch sizes. Iteration stays on owned/adjacent records.
        let edge_work = tree_work(self.facts.len())
            .saturating_add(tree_work(self.jobs.len()))
            .saturating_add(tree_work(self.scopes.len()))
            .saturating_add(6);
        let mut cost = generation
            .assumptions
            .len()
            .saturating_mul(edge_work)
            .saturating_add(generation.facts.len())
            .saturating_add(generation.jobs.len());
        for id in &generation.jobs {
            let job = &self.jobs[id.index];
            cost = cost
                .saturating_add(
                    job.reads
                        .len()
                        .saturating_add(job.provides.len())
                        .saturating_mul(edge_work),
                )
                .saturating_add(self.worklist.removal_work())
                .saturating_add(2);
        }
        for id in &generation.facts {
            let fact = &self.facts[id.index];
            cost = cost.saturating_add(
                fact.readers
                    .len()
                    .saturating_add(fact.providers.len())
                    .saturating_add(fact.assumptions.len())
                    .saturating_mul(edge_work),
            );
        }
        Ok(cost)
    }

    pub(super) fn install_retirement(&mut self, scope: Scope) {
        self.scopes[scope.index].active = false;
        for id in std::mem::take(&mut self.scopes[scope.index].jobs) {
            let job = &mut self.jobs[id.index];
            job.state = JobState::Retired;
            job.task = None;
            self.worklist.satisfy(id);
            for (fact, _) in std::mem::take(&mut job.reads) {
                self.facts[fact.index].readers.remove(&id);
            }
            for fact in std::mem::take(&mut job.provides) {
                self.facts[fact.index].providers.remove(&id);
            }
        }
        self.touch();
    }

    pub(super) fn release_generation(&mut self, scope: Scope) {
        for id in std::mem::take(&mut self.scopes[scope.index].assumptions) {
            self.facts[id.index].assumptions.remove(&scope);
        }
        for id in std::mem::take(&mut self.scopes[scope.index].facts) {
            let fact = &mut self.facts[id.index];
            for reader in std::mem::take(&mut fact.readers) {
                self.jobs[reader.index].reads.remove(&id);
            }
            for provider in std::mem::take(&mut fact.providers) {
                self.jobs[provider.index].provides.remove(&id);
            }
            for target in std::mem::take(&mut fact.assumptions) {
                self.scopes[target.index].assumptions.remove(&id);
            }
        }
        self.touch();
    }

    /// Drop only this retired scope's typed cells after reserving every key and
    /// possible last-owner value destruction. Refusal/cancellation changes no
    /// cells. Matching scope/fact tombstones remain, preventing resurrection and
    /// aliasing with an abandoned fork. Retirement has already woken/removed all
    /// dependencies, so reclamation never clears live facts or notifications.
    pub fn reclaim_channel_scope<Q, P, M>(
        &mut self,
        channel: &mut Channel<Q, P>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>>
    where
        Q: Capability,
        P: PublicationPolicy<Value = Q::Value>,
        M: Meter,
    {
        self.check_channel(channel)?;
        if retired.epoch != self.epoch {
            return Err(KernelError::WrongEpoch);
        }
        let generation = self
            .scopes
            .get(retired.index)
            .filter(|g| g.scope == retired)
            .ok_or(KernelError::InactiveScope)?;
        if generation.active {
            return Err(KernelError::ScopeNotRetired);
        }
        reserve(meter, tree_work(channel.scopes.len()))?;
        let Some(table) = channel.scopes.get(&retired) else {
            return Ok(0);
        };
        reserve(meter, table.cells.len().saturating_add(1))?;
        let work = table.cells.iter().fold(
            tree_work(channel.scopes.len()).saturating_add(2),
            |work, (key, cell)| {
                let drop_work = match &cell.state {
                    Availability::Value(value) => Q::drop_work(value),
                    _ => 1,
                };
                work.saturating_add(Q::key_work(key))
                    .saturating_add(drop_work)
                    .saturating_add(2)
            },
        );
        reserve(meter, work)?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        let count = table.cells.len();
        channel.scopes.remove(&retired);
        channel.len -= count;
        self.touch();
        Ok(count)
    }
}
