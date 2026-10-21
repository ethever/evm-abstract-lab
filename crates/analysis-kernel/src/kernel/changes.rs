//! Prepaid atomic publication, targeted invalidation and ordinary notifications.
//!
//! The plan follows reverse semantic edges only. An affected generation is
//! visited once, then its validity head propagates to downstream adopters. A
//! routing change or descending refinement has an empty semantic seed set. Every
//! plan allocation/traversal is paid before work; no graph mutation precedes the
//! last reservation and cancellation checkpoint. Identity tombstones are inert.

use super::{FactId, JobState, Kernel, Meter, reserve, tree_work};
use crate::{KernelError, Scope};
use std::collections::BTreeSet;

pub(super) struct Changes {
    facts: BTreeSet<FactId>,
    stale: BTreeSet<Scope>,
}

impl<J> Kernel<J> {
    fn collect_assumers<M: Meter>(
        &self,
        id: FactId,
        local_feedback: Option<Scope>,
        stale: &mut BTreeSet<Scope>,
        pending: &mut BTreeSet<Scope>,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        let assumptions = &self.facts[id.index].assumptions;
        // Each scope can enter both sets at most once. The epoch's scope count
        // bounds lookup/insert/pop cost without scanning those scope records.
        let edge_work = tree_work(self.scopes.len())
            .saturating_mul(4)
            .saturating_add(9); // includes the local directory-feedback filter
        reserve(
            meter,
            assumptions
                .len()
                .saturating_mul(edge_work)
                .saturating_add(4), // semantic seed / feedback classification
        )?;
        for scope in assumptions {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            let generation = &self.scopes[scope.index];
            if Some(*scope) != local_feedback
                && generation.active
                && !generation.stale
                && generation.entry.is_some()
                && stale.insert(*scope)
            {
                pending.insert(*scope);
            }
        }
        Ok(())
    }

    pub(super) fn prepare_changes<M: Meter>(
        &self,
        ids: &[FactId],
        semantic: &[FactId],
        local_topology: Option<(FactId, Scope)>,
        meter: &mut M,
        extra: usize,
    ) -> Result<Changes, KernelError<M::Stop>> {
        let mut stale = BTreeSet::new();
        let mut pending = BTreeSet::new();
        for id in semantic {
            self.check_fact(*id)?;
            // Only this publication's own directory feedback is exempt. Foreign
            // membership/family additions, value premises and transitive source
            // invalidation retain their ordinary semantic force.
            let local_feedback = local_topology
                .filter(|(directory, _)| directory == id)
                .map(|(_, scope)| scope);
            self.collect_assumers(*id, local_feedback, &mut stale, &mut pending, meter)?;
        }
        while let Some(scope) = pending.pop_first() {
            self.collect_assumers(
                self.scopes[scope.index].head,
                None,
                &mut stale,
                &mut pending,
                meter,
            )?;
        }
        let maximum = stale.len().saturating_mul(2).saturating_add(ids.len());
        reserve(
            meter,
            maximum
                .saturating_mul(tree_work(maximum).saturating_add(3))
                .saturating_add(1),
        )?;
        let mut facts: BTreeSet<_> = ids.iter().copied().collect();
        let mut cost = extra;
        for scope in &stale {
            let generation = &self.scopes[scope.index];
            facts.insert(generation.head);
            facts.insert(generation.entry.expect("selected assumption owner"));
            // All obsolete local notifications are removed as part of the same
            // transaction. The stale scope itself retains a closure obligation.
            cost = cost.saturating_add(
                generation
                    .jobs
                    .len()
                    .saturating_mul(self.worklist.removal_work().saturating_add(2)),
            );
        }
        let mut wakes = 0usize;
        for id in &facts {
            self.check_fact(*id)?;
            if self.facts[id.index].version.next().is_none() {
                return Err(KernelError::VersionExhausted);
            }
            wakes = wakes.saturating_add(self.facts[id.index].readers.len());
            cost = cost.saturating_add(2);
        }
        // Duplicate readers and stale readers are deliberately over-reserved.
        // Batch enqueue pays geometric vector copying before the first wake.
        cost = cost
            .saturating_add(self.worklist.enqueue_work_for(wakes))
            .saturating_add(wakes.saturating_mul(3));
        reserve(meter, cost)?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        Ok(Changes { facts, stale })
    }

    pub(super) fn install_changes(&mut self, changes: Changes) {
        for scope in &changes.stale {
            self.scopes[scope.index].stale = true;
            for id in &self.scopes[scope.index].jobs {
                self.worklist.satisfy(*id);
            }
        }
        for id in changes.facts {
            let fact = &mut self.facts[id.index];
            fact.version = fact.version.next().expect("preflight checked versions");
            fact.ready = true;
            for reader in &fact.readers {
                self.stats.wake_attempts = self.stats.wake_attempts.saturating_add(1);
                let job = &self.jobs[reader.index];
                let scope = &self.scopes[job.scope.index];
                if job.state != JobState::Retired && scope.active && !scope.stale {
                    self.worklist.enqueue(*reader);
                }
            }
        }
        self.touch();
    }
}
