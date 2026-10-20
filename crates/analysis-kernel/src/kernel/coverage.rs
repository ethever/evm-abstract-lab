//! Stable producer coverage for facts published outside a selected scope.
//!
//! The stable selector identifies a producer family; a job ID or retired scope
//! alone cannot. One last-published scope per family guards the retained numeric
//! bound. Every declared family must cover its current input before that bound
//! is usable. Refreshing the same family restores availability without seeding
//! a numerical-growth invalidation wave. New families invalidate old adopters
//! once, since they could not previously have adopted that producer's premises.

use super::{Fact, FactId, JobId, Kernel, Meter, reserve, tree_work};
use crate::{KernelError, Scope};

pub(super) struct CoverageUpdate {
    pub(super) fact: FactId,
    entry: FactId,
    scope: Scope,
    pub(super) new_family: bool,
    pub(super) work: usize,
}

impl<J> Kernel<J> {
    pub(super) fn producer_ready(&self, entry: FactId, published: Scope) -> bool {
        self.facts.get(entry.index).is_some_and(|fact| {
            fact.id == entry
                && fact.selected == Some(published)
                && self.scopes.get(published.index).is_some_and(|generation| {
                    generation.scope == published
                        && generation.active
                        && !generation.stale
                        && generation.entry == Some(entry)
                })
        })
    }

    // Used by caller-funded introspection and by the prepaid report traversal.
    pub(super) fn coverage_ready(&self, fact: &Fact) -> bool {
        fact.coverage
            .iter()
            .all(|(entry, published)| self.producer_ready(*entry, *published))
    }

    pub(super) fn prepare_coverage<M: Meter>(
        &self,
        job: JobId,
        fact: FactId,
        meter: &mut M,
    ) -> Result<Option<CoverageUpdate>, KernelError<M::Stop>> {
        let scope = self.jobs[job.index].scope;
        let Some(entry) = self.scopes[scope.index].entry else {
            return Ok(None);
        };
        if self.facts[fact.index].scope == Some(scope) {
            return Ok(None);
        }
        reserve(
            meter,
            tree_work(self.facts[fact.index].coverage.len()).saturating_add(1),
        )?;
        let old = self.facts[fact.index].coverage.get(&entry);
        if old == Some(&scope) {
            return Ok(None);
        }
        Ok(Some(CoverageUpdate {
            fact,
            entry,
            scope,
            new_family: old.is_none(),
            work: tree_work(self.facts[fact.index].coverage.len()).saturating_add(3),
        }))
    }

    pub(super) fn install_coverage(&mut self, update: Option<CoverageUpdate>) {
        if let Some(update) = update {
            self.facts[update.fact.index]
                .coverage
                .insert(update.entry, update.scope);
        }
    }

    /// Always retain route subscriptions, including while a family is stale or
    /// has moved to an as-yet-unpublished generation. No semantic premises are
    /// adopted here; callers do that only after all guards and clone work pass.
    pub(super) fn observe_producers<M: Meter>(
        &mut self,
        job: JobId,
        fact: FactId,
        meter: &mut M,
    ) -> Result<bool, KernelError<M::Stop>> {
        let coverage = &self.facts[fact.index].coverage;
        if coverage.is_empty() {
            return Ok(true);
        }
        reserve(meter, coverage.len().saturating_mul(6).saturating_add(1))?;
        let coverage: Vec<_> = coverage
            .iter()
            .map(|(entry, scope)| (*entry, *scope))
            .collect();
        let mut ready = true;
        for (entry, published) in coverage {
            self.observe(job, entry, meter)?;
            ready &= self.producer_ready(entry, published);
        }
        Ok(ready)
    }
}
