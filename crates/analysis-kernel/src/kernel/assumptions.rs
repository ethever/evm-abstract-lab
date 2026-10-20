//! Semantic assumptions are stronger than ordinary wake dependencies.
//!
//! Selected generations retain the union of actually adopted external values
//! (and topology observations). A source head records validity dependencies so
//! invalidation traverses this graph, including descending guarantees. Pending,
//! absence and selector routing never become semantic assumptions. Register the
//! authoritative entry before running rules; no retroactive read scan is needed.

use super::{FactId, JobId, Kernel, Meter, reserve, tree_work};
use crate::{KernelError, Scope};

impl<J> Kernel<J> {
    pub(super) fn assume<M: Meter>(
        &mut self,
        job: JobId,
        fact: FactId,
        source: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        let target = self.jobs[job.index].scope;
        if source == target || self.scopes[target.index].entry.is_none() {
            return Ok(());
        }
        self.check_usable_scope(target)?;
        self.check_usable_scope(source)?;
        self.check_fact(fact)?;
        let head = self.scopes[source.index].head;
        let cost = [fact, head].iter().fold(1usize, |cost, id| {
            cost.saturating_add(tree_work(self.facts[id.index].assumptions.len()))
                .saturating_add(tree_work(
                    self.scopes[target.index]
                        .assumptions
                        .len()
                        .saturating_add(2),
                ))
                .saturating_add(3)
        });
        reserve(meter, cost)?;
        meter.checkpoint().map_err(KernelError::Stopped)?;
        for id in [fact, head] {
            self.facts[id.index].assumptions.insert(target);
            self.scopes[target.index].assumptions.insert(id);
        }
        self.touch();
        Ok(())
    }
}
