//! Paid local mandatory closure from live ownership/required-demand indexes.
//!
//! Do not walk epoch history or cache a second readiness truth. Deduplicate this
//! scope's required facts only, then quote producer adjacency before consulting
//! the same effective availability used by global introspection.
use super::{JobState, Kernel, Meter, reserve, tree_work};
use crate::{Closure, KernelError, Open, Scope};
use std::collections::BTreeSet;

impl<J> Kernel<J> {
    /// Scope-relative mandatory closure with prepaid visits to owned live jobs,
    /// deduplicated required facts/producer guards and unresolved obligations.
    /// Unrelated historical records add no traversal or linear charge. Refusal
    /// and cancellation mutate only the caller's cumulative meter, not the graph.
    pub fn closure_in_metered<M: Meter>(
        &self,
        scope: Scope,
        meter: &mut M,
    ) -> Result<Closure, KernelError<M::Stop>> {
        self.check_scope(scope)?;
        let generation = &self.scopes[scope.index];
        reserve(meter, 2)?;
        let mut open = Open {
            unsealed: usize::from(!generation.sealed),
            obligations: generation
                .open_obligations
                .len()
                .checked_add(usize::from(generation.stale))
                .ok_or(KernelError::CostOverflow)?,
            ..Open::default()
        };
        // Invalidation atomically removes all local queue obligations; obsolete
        // in-flight jobs and their demands do not add work until owner reseeding.
        if generation.stale {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            return Ok(Closure::Open(open));
        }
        reserve(meter, generation.jobs.len().saturating_add(1))?;
        let edges = generation.jobs.iter().try_fold(0usize, |count, id| {
            count
                .checked_add(self.jobs[id.index].required.len())
                .ok_or(KernelError::CostOverflow)
        })?;
        let work = edges
            .saturating_mul(tree_work(edges).saturating_add(3))
            .saturating_add(
                generation
                    .jobs
                    .len()
                    .saturating_mul(tree_work(self.worklist.len()).saturating_add(8)),
            )
            .saturating_add(1);
        reserve(meter, work)?;
        let mut required = BTreeSet::new();
        for id in &generation.jobs {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            let job = &self.jobs[id.index];
            if job.state == JobState::Retired {
                continue;
            }
            open.pending += usize::from(self.worklist.contains(id));
            match job.state {
                JobState::Admitted | JobState::Running | JobState::Suspended => open.in_flight += 1,
                JobState::Interrupted => open.interrupted += 1,
                _ => {}
            }
            required.extend(job.required.iter().copied());
        }
        reserve(meter, required.len().saturating_add(1))?;
        let guards = required.iter().try_fold(0usize, |count, id| {
            count
                .checked_add(self.facts[id.index].coverage.len())
                .ok_or(KernelError::CostOverflow)
        })?;
        reserve(
            meter,
            required
                .len()
                .saturating_mul(12)
                .saturating_add(guards.saturating_mul(8))
                .saturating_add(1),
        )?;
        for id in &required {
            meter.checkpoint().map_err(KernelError::Stopped)?;
            open.missing += usize::from(!self.fact_ready(&self.facts[id.index]));
        }
        meter.checkpoint().map_err(KernelError::Stopped)?;
        Ok(if open.is_empty() {
            Closure::Closed
        } else {
            Closure::Open(open)
        })
    }
}
