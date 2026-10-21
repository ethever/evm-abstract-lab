//! Wake edges retain a monotone required subset for closure. Discovery never
//! downgrades an earlier demand, and producer/validity subscriptions use the same
//! mode as the originating observation. Semantic premises live separately.
use super::{FactId, JobId, Kernel, Meter, ReadMode, reserve, tree_work};
use crate::{KernelError, Scope};

impl<J> Kernel<J> {
    pub(super) fn observe<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.observe_mode(job, id, ReadMode::Required, meter)
    }
    pub(super) fn observe_mode<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        mode: ReadMode,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_job(job)?;
        self.check_fact(id)?;
        let mut cost = tree_work(self.jobs[job.index].reads.len())
            .saturating_mul(2)
            .saturating_add(tree_work(self.facts[id.index].readers.len()))
            .saturating_add(2);
        if mode == ReadMode::Required {
            cost = cost
                .saturating_add(tree_work(self.jobs[job.index].required.len()))
                .saturating_add(tree_work(self.facts[id.index].demands.len()))
                .saturating_add(2);
        }
        reserve(meter, cost)?;
        let new = self.jobs[job.index]
            .reads
            .insert(id, self.facts[id.index].version)
            .is_none();
        if new {
            self.facts[id.index].readers.insert(job);
        }
        if mode == ReadMode::Required {
            self.jobs[job.index].required.insert(id);
            self.facts[id.index].demands.insert(job);
        }
        self.touch();
        Ok(())
    }
    pub(super) fn observe_scoped<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        scope: Scope,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.observe_scoped_mode(job, id, scope, ReadMode::Required, meter)
    }
    pub(super) fn observe_scoped_mode<M: Meter>(
        &mut self,
        job: JobId,
        id: FactId,
        scope: Scope,
        mode: ReadMode,
        meter: &mut M,
    ) -> Result<(), KernelError<M::Stop>> {
        self.check_scope(scope)?;
        self.observe_mode(job, self.scopes[scope.index].head, mode, meter)?;
        self.observe_mode(job, id, mode, meter)
    }
}
