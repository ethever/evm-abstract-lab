//! One selected proof for whole-relative-table completion. Native MAY payloads
//! remain stored; kernel producer coverage guards the current closure proof.
use super::{Composition, Cooperation};
use crate::{
    analysis::{
        MachineError,
        cooperation::{
            CooperationError, InputTasks, PointKind, ProcedureId, Summary, SummaryClosure,
        },
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task, TaskKind},
        },
    },
    domain::WordElement,
    resource::WorkBudget,
};
use analysis_kernel::{
    Availability, Change, GenerationEntry, JobId, KernelError, LocalClosure, Meter, Run,
    RunningJob, Scope, ScopeKey, Scoped,
};
use std::sync::Arc;

pub(super) struct Proof {
    entry: GenerationEntry,
    pub(super) scope: Scope,
    job: JobId,
    owner: JobId,
}

impl<N: WordElement, X: Composition<N, E>, E> Cooperation<N, X, E> {
    /// New native sources/edges and previously unseeded contribution families
    /// change the proof basis even before any old scalar slot version changes.
    pub(in crate::analysis) fn structure_changed(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        producer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        if !X::ENABLED {
            return Ok(());
        }
        if self.structure.is_none() {
            let scope = runtime.relative;
            let slot = runtime.kernel().slot(scope, &mut MachineMeter(budget))?;
            self.structure = Some((slot, 0));
        }
        let (slot, revision) = self.structure.as_mut().ok_or(KernelError::InvalidHandle)?;
        let next = revision
            .checked_add(1)
            .ok_or(KernelError::VersionExhausted)?;
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(producer, &mut meter)?;
        run.publish_prepared_with(slot, Change::Changed, 1, || *revision = next)?;
        *producer = run.suspend();
        Ok(())
    }

    fn proof_job(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        producer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(JobId, Scope), CooperationError<E>> {
        let owner = self.consumer_job(runtime, producer, budget)?;
        if !matches!(runtime.view().task(owner)?.0, TaskKind::SummaryObservation) {
            return Err(KernelError::InvalidJob.into());
        }
        if self
            .proof
            .as_ref()
            .is_some_and(|proof| proof.owner != owner)
        {
            return Err(KernelError::InvalidHandle.into());
        }
        let refresh = match &self.proof {
            Some(proof) => runtime
                .view()
                .generation_stale(&proof.entry, &mut MachineMeter(budget))?,
            None => true,
        };
        if refresh {
            let (context, generation) = if let Some(proof) = &self.proof {
                let key = proof.scope.key();
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
            let mut meter = MachineMeter(budget);
            let scope = runtime.kernel().scope(
                ScopeKey {
                    context,
                    generation,
                    policy: 0,
                },
                &mut meter,
            )?;
            let job = runtime
                .kernel()
                .register(scope, Task(TaskKind::SummaryProof), &mut meter)?;
            runtime.kernel().seal_scope(scope, &mut meter)?;
            let entry = if let Some(proof) = &mut self.proof {
                let mut run = runtime.kernel().resume(producer, &mut meter)?;
                run.replace_generation(&mut proof.entry, scope)?;
                *producer = run.suspend();
                proof.entry.clone()
            } else {
                let owner_scope = runtime.view().job_scope(owner)?;
                runtime
                    .kernel()
                    .generation_entry(owner_scope, scope, &mut meter)?
            };
            self.proof = Some(Proof {
                entry,
                scope,
                job,
                owner,
            });
        }
        let proof = self.proof.as_ref().ok_or(KernelError::InvalidHandle)?;
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(producer, &mut meter)?;
        run.reserve(self.bindings.len().saturating_mul(8).saturating_add(1))?;
        // The permanent producer is woken as soon as any actual basis loses
        // validity; it never waits for a subsequent numerical output update.
        run.current_generation(&proof.entry)?;
        // A Pending read adopts no source head. Track routing/availability on
        // the permanent owner too, so readiness-only wakes run this owner and
        // consume the internal proof job synchronously on the same Kernel.
        for binding in self
            .bindings
            .values()
            .filter(|binding| binding.input.subject().point.kind == PointKind::Body)
        {
            let current = run.current_generation(&binding.entry)?;
            if let Availability::Value(scope) = current.state {
                run.query_discovery(
                    &mut self
                        .shared
                        .as_mut()
                        .ok_or(KernelError::InvalidHandle)?
                        .inputs,
                    Scoped::new(scope, ()),
                )?;
            }
        }
        *producer = run.suspend();
        Ok((proof.job, proof.scope))
    }

    pub(in crate::analysis) fn publish_summaries(
        &mut self,
        summaries: Vec<(ProcedureId, Summary<N>)>,
        closed: bool,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        producer: &mut RunningJob,
        budget: &mut WorkBudget,
        observe_native: impl FnOnce(
            &mut Run<'_, Task<<X as InputTasks>::Task>, MachineMeter<'_>>,
        ) -> Result<(), KernelError<MachineError>>,
    ) -> Result<(), CooperationError<E>> {
        // Snapshot publication belongs to the permanent native owner, never
        // the selected completion proof. First Value can unblock ordinary MAY
        // readers even while native/body completion is still Pending.
        let owner = self.consumer_job(runtime, producer, budget)?;
        if !matches!(runtime.view().task(owner)?.0, TaskKind::SummaryObservation) {
            return Err(KernelError::InvalidJob.into());
        }
        let mut generations = Vec::new();
        {
            let mut meter = MachineMeter(budget);
            meter
                .reserve(summaries.len().saturating_mul(8).saturating_add(1))
                .map_err(KernelError::Stopped)?;
            generations.reserve(summaries.len());
            let universe = runtime.world;
            let mut run = runtime.kernel().resume(producer, &mut meter)?;
            let shared = self.shared.as_mut().ok_or(KernelError::InvalidHandle)?;
            for (id, summary) in summaries {
                generations.push((id, summary.generation));
                run.publish(
                    &mut shared.summaries,
                    Scoped::new(universe, id),
                    Arc::new(summary),
                )
                .map_err(super::observations::publication_error)?;
            }
            *producer = run.suspend();
        }
        if self.structure.is_none() {
            let relative = runtime.relative;
            let slot = runtime.kernel().slot(relative, &mut MachineMeter(budget))?;
            self.structure = Some((slot, 0));
        }
        let (job, scope) = self.proof_job(runtime, producer, budget)?;
        let mut meter = MachineMeter(budget);
        meter
            .reserve(
                self.bindings.len().saturating_mul(8).saturating_add(
                    generations
                        .len()
                        .saturating_mul(((usize::BITS as usize) + 1) * 16),
                ),
            )
            .map_err(KernelError::Stopped)?;
        runtime.kernel().retry(job, &mut meter)?;
        let universe = runtime.world;
        let mut run = runtime.kernel().begin(job, &mut meter)?;
        if let Some((slot, _)) = &self.structure {
            run.observe_slot(slot)?;
        }
        observe_native(&mut run)?;
        let shared = self.shared.as_mut().ok_or(KernelError::InvalidHandle)?;
        let mut ready = closed;
        // One pass for the table's actual current execution/contribution
        // scopes. Input adoption carries their cumulative foreign premises,
        // including guarantees used for complete-empty conclusions.
        for binding in self
            .bindings
            .values()
            .filter(|binding| binding.input.subject().point.kind == PointKind::Body)
        {
            let scope = binding.input.subject().scope;
            let (kernel, meter) = run.parts();
            if kernel.scope_stale(scope, meter)? {
                ready = false;
                continue;
            }
            let input = run.query(&mut shared.inputs, Scoped::new(scope, ()))?;
            ready &= matches!(input.state, Availability::Value(_));
        }
        if ready {
            for (id, generation) in generations {
                // Only an actual closed proof can restore this family coverage.
                // Same native generation reuses the exact marker without a
                // synthetic observation revision or another payload snapshot.
                let published = run.publish(
                    &mut shared.closures,
                    Scoped::new(universe, id),
                    SummaryClosure { generation },
                );
                let (kernel, meter) = run.parts();
                meter.checkpoint().map_err(KernelError::Stopped)?;
                if kernel.scope_stale(scope, meter)? {
                    // Publication can retire its own proof through real feedback.
                    // The selector already dirtied the persistent native owner.
                    return Ok(());
                }
                published.map_err(super::observations::publication_error)?;
            }
        }
        run.finish(LocalClosure::Stable)?;
        Ok(())
    }
}
