//! Complete native producer inventories and bounded generation ownership.
use super::{Composition, Cooperation, Dormant};
use crate::{
    analysis::{
        cooperation::{CooperationError, InputTasks, Point, Subject},
        worklist::{MachineMeter, runtime::Runtime},
    },
    domain::WordElement,
    resource::WorkBudget,
};
use analysis_kernel::{JobId, KernelError, Meter, ReclaimScope, RunningJob, Scope, ScopeKey};

impl<N: WordElement, X: Composition<N, Error = E>, E> Cooperation<N, X, E> {
    /// Share only allocation lineage; each producer still owns its distinct
    /// scope inventory, seeding, selector publication and retirement protocol.
    pub(super) fn next_scope_key(
        &mut self,
        previous: Option<Scope>,
    ) -> Result<ScopeKey, CooperationError<E>> {
        let (context, generation) = if let Some(previous) = previous {
            let key = previous.key();
            (
                key.context,
                key.generation
                    .checked_add(1)
                    .ok_or(KernelError::VersionExhausted)?,
            )
        } else {
            let context = self.next_context;
            self.next_context = context
                .checked_add(1)
                .ok_or(KernelError::VersionExhausted)?;
            (context, 1)
        };
        Ok(ScopeKey {
            context,
            generation,
            policy: 0,
        })
    }

    /// Validate the receipt actually consumed, rather than a replacement scope
    /// selected since execution. Required reads installed its semantic premises.
    pub(super) fn receipt_valid(
        &self,
        receipt: Subject,
        runtime: &Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<bool, CooperationError<E>> {
        self.receipt_valid_metered(receipt, runtime, &mut MachineMeter(budget))
    }

    pub(super) fn receipt_valid_metered<M: Meter<Stop = crate::analysis::MachineError>>(
        &self,
        receipt: Subject,
        runtime: &Runtime<<X as InputTasks>::Task>,
        meter: &mut M,
    ) -> Result<bool, CooperationError<E>> {
        meter
            .reserve(((usize::BITS as usize) + 1) * 8)
            .map_err(KernelError::Stopped)?;
        let Some(binding) = self.bindings.get(&receipt.point) else {
            return Ok(false);
        };
        if binding.input.subject() != receipt
            || runtime.view().scope_stale(receipt.scope, meter)?
            || runtime.view().generation_stale(&binding.entry, meter)?
        {
            return Ok(false);
        }
        Ok(super::clients_ready(
            runtime.view().closure_in_metered(receipt.scope, meter)?,
        ))
    }

    pub(super) fn consumer_job(
        &self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<JobId, CooperationError<E>> {
        let mut meter = MachineMeter(budget);
        let run = runtime.kernel().resume(consumer, &mut meter)?;
        let job = run.job();
        *consumer = run.suspend();
        Ok(job)
    }

    pub(in crate::analysis) fn begin_owner(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 8)
            .map_err(KernelError::Stopped)?;
        let job = self.consumer_job(runtime, consumer, budget)?;
        if let Some(owner) = super::source::Owner::of(&runtime.view().task(job)?.0) {
            self.sources.remove(&owner);
        }
        let old = self.owner_epochs.get(&job).copied().unwrap_or(0);
        self.owner_epochs.insert(
            job,
            old.checked_add(1).ok_or(KernelError::VersionExhausted)?,
        );
        Ok(())
    }

    pub(super) fn defer_owner(
        &mut self,
        job: JobId,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 8)
            .map_err(KernelError::Stopped)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = self.deferred.entry(job) {
            let scope = runtime.view().job_scope(job)?;
            let obligation = runtime
                .kernel()
                .obligation(scope, &mut MachineMeter(budget))?;
            entry.insert(obligation);
        }
        Ok(())
    }

    /// Resolve only the native deferred-transfer obligation. Explicit extension
    /// obligations remain in their own client scopes and still block the joint
    /// fixed point; they cannot cyclically block a sound coarse native summary.
    pub(in crate::analysis) fn complete_native(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<bool, CooperationError<E>> {
        if !X::ENABLED {
            return Ok(true);
        }
        let job = self.consumer_job(runtime, consumer, budget)?;
        let epoch = self.owner_epochs.get(&job).copied().unwrap_or(0);
        let points = self.owner_points.get(&job);
        MachineMeter(budget)
            .reserve(
                points
                    .map_or(0, |points| points.len())
                    .saturating_mul(((usize::BITS as usize) + 1) * 8),
            )
            .map_err(KernelError::Stopped)?;
        for point in points.into_iter().flatten() {
            let binding = self.bindings.get(point).ok_or(KernelError::InvalidHandle)?;
            if binding.consumer == job
                && (point.namespace != 2 || binding.producer == Some((job, epoch)))
                && runtime
                    .view()
                    .generation_stale(&binding.entry, &mut MachineMeter(budget))?
            {
                return Ok(false);
            }
        }
        if let Some(obligation) = self.deferred.get(&job).copied() {
            runtime
                .kernel()
                .resolve_obligation(obligation, &mut MachineMeter(budget))?;
            self.deferred.remove(&job);
        }
        Ok(true)
    }

    pub(super) fn reclaim_retired(
        &mut self,
        point: Point,
        retired: Scope,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        let native_work = self
            .bindings
            .get(&point)
            .map_or(0, |binding| binding.input.work_size());
        let local_work = self
            .stores
            .get(&retired)
            .map_or(0, |local| self.assembly.retirement_work(local));
        MachineMeter(budget)
            .reserve(native_work.saturating_add(local_work))
            .map_err(KernelError::Stopped)?;
        let shared = self.shared.as_mut().ok_or(KernelError::InvalidHandle)?;
        runtime.kernel().reclaim_channel_scope(
            &mut shared.inputs,
            retired,
            &mut MachineMeter(budget),
        )?;
        runtime.kernel().reclaim_channel_scope(
            &mut shared.refinements,
            retired,
            &mut MachineMeter(budget),
        )?;
        runtime.kernel().reclaim_channel_scope(
            &mut shared.process_refinements,
            retired,
            &mut MachineMeter(budget),
        )?;
        // A shared client provider may also contain generation-scoped cells.
        // Each typed channel reclaims only this retired scope; world aggregate
        // cells keep their values. Failure remains an unfinished owner action.
        self.global
            .as_mut()
            .ok_or(KernelError::InvalidHandle)?
            .reclaim_scope(runtime.kernel(), retired, &mut MachineMeter(budget))?;
        self.stores.remove(&retired);
        self.scope_owners.remove(&retired);
        Ok(())
    }

    /// Call only after complete native transfer/admission and an unchanged
    /// native source-version receipt. Stale or required-unfinished assumptions
    /// prevent withdrawal, even if the interpreter emitted no new frontier.
    pub(in crate::analysis) fn finish_owner(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
        receipt: Option<Subject>,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        if let Some(receipt) = receipt
            && !self.receipt_valid(receipt, runtime, budget)?
        {
            return Ok(());
        }
        if !self.complete_native(runtime, consumer, budget)? {
            return Ok(());
        }
        let job = self.consumer_job(runtime, consumer, budget)?;
        let epoch = *self.owner_epochs.get(&job).ok_or(KernelError::InvalidJob)?;
        let points = self.owner_points.get(&job);
        MachineMeter(budget)
            .reserve(
                points
                    .map_or(0, |points| points.len())
                    .saturating_mul(((usize::BITS as usize) + 1) * 16),
            )
            .map_err(KernelError::Stopped)?;
        for point in points.into_iter().flatten() {
            let binding = self.bindings.get(point).ok_or(KernelError::InvalidHandle)?;
            // Old unvisited contributions are the candidates being closed.
            // The native view and current visited contributions are premises.
            if binding.consumer == job
                && (point.namespace != 2 || binding.producer == Some((job, epoch)))
            {
                if runtime
                    .view()
                    .generation_stale(&binding.entry, &mut MachineMeter(budget))?
                {
                    return Ok(());
                }
                if runtime
                    .view()
                    .closure_in_metered(binding.input.subject().scope, &mut MachineMeter(budget))?
                    != analysis_kernel::Closure::Closed
                {
                    return Ok(());
                }
            }
        }
        let closed: Vec<_> = points
            .into_iter()
            .flatten()
            .filter_map(|point| {
                self.bindings
                    .get(point)
                    .and_then(|binding| binding.producer)
                    .filter(|(producer, seen)| *producer == job && *seen != epoch)
                    .map(|_| *point)
            })
            .collect();
        for point in closed {
            // Withdrawal can itself invalidate an adopted source premise.
            // Do not certify the remaining omissions using that old receipt.
            if let Some(receipt) = receipt
                && !self.receipt_valid(receipt, runtime, budget)?
            {
                return Ok(());
            }
            let old = self
                .bindings
                .get(&point)
                .ok_or(KernelError::InvalidHandle)?;
            let retired = old.input.subject().scope;
            let key = retired.key();
            let next_generation = key
                .generation
                .checked_add(1)
                .ok_or(KernelError::VersionExhausted)?;
            {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().resume(consumer, &mut meter)?;
                let binding = self
                    .bindings
                    .get_mut(&point)
                    .ok_or(KernelError::InvalidHandle)?;
                run.withdraw_generation(&mut binding.entry)?;
                *consumer = run.suspend();
            }
            self.reclaim_retired(point, retired, runtime, budget)?;
            let binding = self
                .bindings
                .remove(&point)
                .ok_or(KernelError::InvalidHandle)?;
            if let Some(points) = self.owner_points.get_mut(&job) {
                points.remove(&point);
            }
            // Only stable identity and allocation lineage remain; no native
            // snapshot, local guarantee or client store survives withdrawal.
            self.dormant.insert(
                point,
                Dormant {
                    entry: binding.entry,
                    context: key.context,
                    next_generation,
                },
            );
        }
        self.record_source(runtime, consumer, budget, receipt)
    }
}
