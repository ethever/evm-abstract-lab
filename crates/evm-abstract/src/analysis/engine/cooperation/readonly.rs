//! Read-only native entry snapshots owned by a persistent observation producer.
use super::{Composition, Cooperation};
use crate::{
    analysis::{
        EvmMachinePayload, MachineError,
        cooperation::{
            self as public, CooperationError, Input, InputTasks, Point, PointKind, ProcedureId,
            Subject,
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
    GenerationEntry, JobId, KernelError, LocalClosure, Meter, RunningJob, ScopeKey, SlotMeta,
    Version,
};

pub(super) struct Binding<N: WordElement> {
    revision: usize,
    anchor: Version,
    source: SlotMeta<usize>,
    pub(super) owner: JobId,
    pub(super) input: Input<N>,
    pub(super) entry: GenerationEntry,
}

impl<N: WordElement, X: Composition<N, E>, E> Cooperation<N, X, E> {
    /// Publish the actual retained entry, independently of Body keys/callers.
    /// The lazy materializer must prepay its native construction/traversal from
    /// this root budget. It is never called for an unchanged current source.
    /// A failure leaves the real producer in flight and its obligation open.
    #[expect(
        clippy::too_many_arguments,
        reason = "native source identity, persistent owner and original root ledger"
    )]
    pub(in crate::analysis) fn publish_entry_input(
        &mut self,
        id: ProcedureId,
        revision: usize,
        source: &SlotMeta<usize>,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        producer: &mut RunningJob,
        budget: &mut WorkBudget,
        materialize: impl FnOnce(&mut WorkBudget) -> Result<EvmMachinePayload<N>, MachineError>,
    ) -> Result<Subject, CooperationError<E>> {
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 64)
            .map_err(KernelError::Stopped)?;
        let owner = self.consumer_job(runtime, producer, budget)?;
        if !matches!(runtime.view().task(owner)?.0, TaskKind::SummaryObservation) {
            return Err(KernelError::InvalidJob.into());
        }
        if self
            .readonly
            .get(&id)
            .is_some_and(|binding| binding.owner != owner || binding.source != *source)
        {
            return Err(KernelError::InvalidHandle.into());
        }
        self.ensure_shared(runtime, producer, budget)?;
        if self.global.is_none() {
            self.global = Some(
                self.assembly
                    .create_shared(runtime.kernel(), &mut MachineMeter(budget))?,
            );
        }
        let anchor = {
            let mut meter = MachineMeter(budget);
            let mut run = runtime.kernel().resume(producer, &mut meter)?;
            let anchor = run.observe_slot(source)?;
            *producer = run.suspend();
            anchor
        };
        let refresh = match self.readonly.get(&id) {
            Some(binding) => {
                binding.revision != revision
                    || binding.anchor != anchor
                    || runtime
                        .view()
                        .generation_stale(&binding.entry, &mut MachineMeter(budget))?
            }
            None => true,
        };
        if refresh {
            let payload =
                materialize(budget).map_err(public::InitializationError::Materialization)?;
            MachineMeter(budget)
                .reserve(payload.work_size().saturating_mul(2).saturating_add(64))
                .map_err(KernelError::Stopped)?;
            let (context, generation) = if let Some(binding) = self.readonly.get(&id) {
                let key = binding.input.subject().scope.key();
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
            let point = Point {
                namespace: id.namespace,
                kind: PointKind::ProcedureEntry,
                index: id.index,
            };
            let scope = runtime.kernel().scope(
                ScopeKey {
                    context,
                    generation,
                    policy: 0,
                },
                &mut MachineMeter(budget),
            )?;
            let subject = Subject { point, scope };
            let input = Input::new(subject, payload).in_universe(runtime.world);
            // Only this native seed exists here. In particular, do not invoke
            // assembly.tasks/create or the executable input initializer.
            let seed = runtime.kernel().register(
                scope,
                Task(TaskKind::Seed(subject)),
                &mut MachineMeter(budget),
            )?;
            runtime
                .kernel()
                .seal_scope(scope, &mut MachineMeter(budget))?;
            let retired = self
                .readonly
                .get(&id)
                .map(|binding| binding.input.subject().scope);
            let entry = if let Some(binding) = self.readonly.get_mut(&id) {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().resume(producer, &mut meter)?;
                run.replace_generation(&mut binding.entry, scope)?;
                *producer = run.suspend();
                binding.entry.clone()
            } else {
                let owner_scope = runtime.view().job_scope(owner)?;
                runtime
                    .kernel()
                    .generation_entry(owner_scope, scope, &mut MachineMeter(budget))?
            };
            {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().begin(seed, &mut meter)?;
                if run.observe_slot(source)? != anchor {
                    return Err(KernelError::InvalidHandle.into());
                }
                public::initialize_readonly(
                    &mut run,
                    self.shared.as_mut().ok_or(KernelError::InvalidHandle)?,
                    &input,
                )?;
                run.finish(LocalClosure::Stable)?;
            }
            if let Some(retired) = retired {
                // Reclaim pays each shared channel; separately fund the native
                // binding's possible last handle before replacing it.
                let retained = self
                    .readonly
                    .get(&id)
                    .ok_or(KernelError::InvalidHandle)?
                    .input
                    .work_size();
                MachineMeter(budget)
                    .reserve(retained)
                    .map_err(KernelError::Stopped)?;
                self.reclaim_retired(point, retired, runtime, budget)?;
            }
            self.publish_point(point, &entry, runtime, producer, budget)?;
            self.readonly.insert(
                id,
                Binding {
                    revision,
                    anchor,
                    source: source.clone(),
                    owner,
                    input,
                    entry,
                },
            );
        }
        let binding = self.readonly.get(&id).ok_or(KernelError::InvalidHandle)?;
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(producer, &mut meter)?;
        run.current_generation(&binding.entry)?;
        *producer = run.suspend();
        Ok(binding.input.subject())
    }
}
