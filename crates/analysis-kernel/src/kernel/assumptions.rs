//! Generation-wide cumulative semantic premises, separate from wake edges.
//!
//! Actual external values include both their own scope and automatically declared
//! producer validity. All premise insertions for one delivered value are prepaid
//! together; a refused copy/observation cannot adopt a partial usable guarantee.
//! Pending, absence and routing never acquire semantic value premises. Topology
//! observations adopt the directory plus producer validity of resolved members.

use super::{FactId, JobId, Kernel, Meter, Selection, reserve, tree_work};
use crate::{KernelError, Scope};
use std::collections::BTreeSet;

impl<J> Kernel<J> {
    pub(super) fn assume<M: Meter>(
        &mut self,
        job: JobId,
        fact: FactId,
        source: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.adopt(job, fact, Some(source), meter)
    }

    pub(super) fn assume_producers<M: Meter>(
        &mut self,
        job: JobId,
        fact: FactId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.adopt(job, fact, None, meter)
    }

    fn adopt<M: Meter>(
        &mut self,
        job: JobId,
        fact: FactId,
        source: Option<Scope>,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        let target = self.jobs[job.index].scope;
        if self.scopes[target.index].entry.is_none() {
            return Ok(());
        }
        self.check_usable_scope(target)?;
        self.check_fact(fact)?;
        if self.facts[fact.index].coverage.is_empty() && source.is_none_or(|scope| scope == target)
        {
            return Ok(());
        }
        let maximum = self.facts[fact.index].coverage.len().saturating_add(2);
        reserve(
            meter,
            maximum.saturating_mul(tree_work(maximum).saturating_add(12)),
        )?;
        let mut premises = BTreeSet::new();
        if let Some(source) = source {
            self.check_usable_scope(source)?;
            let foreign = self.facts[fact.index]
                .coverage
                .values()
                .any(|producer| *producer != target);
            if source != target || foreign {
                premises.insert(fact);
            }
            if source != target {
                premises.insert(self.scopes[source.index].head);
            }
        }
        for (entry, published) in &self.facts[fact.index].coverage {
            if !self.producer_ready(*entry, *published) {
                return Err(KernelError::PendingCoverage);
            }
            match self.facts[entry.index]
                .selected
                .expect("covered stable entry")
            {
                Selection::Absent { .. } => {
                    premises.insert(*entry);
                }
                Selection::Active(scope) if scope != target => {
                    premises.insert(self.scopes[scope.index].head);
                }
                _ => {}
            }
        }
        let cost = premises.iter().fold(1usize, |cost, id| {
            cost.saturating_add(tree_work(self.facts[id.index].assumptions.len()))
                .saturating_add(tree_work(
                    self.scopes[target.index]
                        .assumptions
                        .len()
                        .saturating_add(premises.len()),
                ))
                .saturating_add(3)
        });
        reserve(meter, cost)?;
        for id in premises {
            self.facts[id.index].assumptions.insert(target);
            self.scopes[target.index].assumptions.insert(id);
        }
        self.touch();
        Ok(())
    }
}
