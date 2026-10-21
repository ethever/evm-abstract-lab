//! Production interpreter adapter for statically assembled kernel analyzers.
//!
//! Ascending machine inputs stay authoritative. Each semantic input version has
//! an immutable snapshot and a freshly seeded descending scope; only a detached
//! execution view consumes external guarantees. Stable generation entries route
//! readers across replacement without joining historical narrowed facts.

use crate::{
    analysis::{
        EvmMachinePayload, MachineError,
        cooperation::{
            self as public, CooperationError, EvmContext, ExtraStores, Input, InputTasks,
            LocalInput, Point, PointKind, Ports, Shared, SharedStores, Subject,
        },
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task, TaskKind},
        },
    },
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement},
    resource::WorkBudget,
};
use analysis_kernel::{
    Analyzer, Availability, Context, GenerationEntry, JobId, KernelError, LocalClosure, RunningJob,
    ScopeKey, Version,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A bound alias only: semantic implementations use the existing kernel traits.
pub(in crate::analysis) trait Composition<N: WordElement, E>:
    InputTasks
    + ExtraStores
    + SharedStores
    + for<'r, 'k, 'm> Analyzer<
        EvmContext<
            'r,
            'k,
            'm,
            N,
            <Self as InputTasks>::Task,
            <Self as ExtraStores>::Stores,
            <Self as SharedStores>::Shared,
        >,
        Task = <Self as InputTasks>::Task,
        Error = E,
    >
{
}
impl<N: WordElement, E, X> Composition<N, E> for X where
    X: InputTasks
        + ExtraStores
        + SharedStores
        + for<'r, 'k, 'm> Analyzer<
            EvmContext<
                'r,
                'k,
                'm,
                N,
                <X as InputTasks>::Task,
                <X as ExtraStores>::Stores,
                <X as SharedStores>::Shared,
            >,
            Task = <X as InputTasks>::Task,
            Error = E,
        >
{
}

/// Borrow the caller's concrete assembly without cloning its mutable state.
pub(super) struct Borrowed<'a, X>(pub &'a mut X);
impl<X: InputTasks> InputTasks for Borrowed<'_, X> {
    type Task = X::Task;
    const ENABLED: bool = X::ENABLED;
    fn registration_work(&self) -> usize {
        self.0.registration_work()
    }
    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        self.0.tasks(subject)
    }
}
impl<X: ExtraStores> ExtraStores for Borrowed<'_, X> {
    type Stores = X::Stores;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        self.0.retirement_work(stores)
    }
    fn create<J, M: analysis_kernel::Meter>(
        &self,
        kernel: &mut analysis_kernel::Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        self.0.create(kernel, meter)
    }
}

impl<X: SharedStores> SharedStores for Borrowed<'_, X> {
    type Shared = X::Shared;
    fn create_shared<J, M: analysis_kernel::Meter>(
        &self,
        kernel: &mut analysis_kernel::Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Shared, KernelError<M::Stop>> {
        self.0.create_shared(kernel, meter)
    }
}

impl<Cx, X: Analyzer<Cx>> Analyzer<Cx> for Borrowed<'_, X> {
    type Task = X::Task;
    type Error = X::Error;
    fn evaluate(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        self.0.evaluate(task, cx)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    anchor: Version,
    phase: u8,
    secondary: Option<Version>,
    parent: Option<Subject>,
}
struct Binding<N: WordElement> {
    stamp: Stamp,
    input: Input<N>,
    entry: GenerationEntry,
    producer: Option<(JobId, usize)>,
    consumer: JobId,
}
struct Dormant {
    entry: GenerationEntry,
    context: u64,
    next_generation: u64,
}

pub(in crate::analysis) struct Cooperation<
    N: WordElement,
    X: InputTasks + ExtraStores + SharedStores,
    E,
> {
    assembly: X,
    stores: BTreeMap<analysis_kernel::Scope, <X as ExtraStores>::Stores>,
    shared: Option<Shared<N>>,
    global: Option<X::Shared>,
    // Publication metadata only; the native result has one visible channel
    // snapshot, and older snapshots are released on publication.
    bindings: BTreeMap<Point, Binding<N>>,
    readonly: BTreeMap<public::ProcedureId, readonly::Binding<N>>,
    proof: Option<proof::Proof>,
    structure: Option<(analysis_kernel::SlotMeta<usize>, usize)>,
    dormant: BTreeMap<Point, Dormant>,
    owner_epochs: BTreeMap<JobId, usize>,
    scope_owners: BTreeMap<analysis_kernel::Scope, JobId>,
    owner_points: BTreeMap<JobId, BTreeSet<Point>>,
    deferred: BTreeMap<JobId, analysis_kernel::ObligationId>,
    next_context: u64,
    failure: Option<CooperationError<E>>,
    pub interrupted: bool,
}

pub(in crate::analysis) type Empty = public::Extension<public::NoExtension, public::NoTasks>;

impl<N: WordElement, X: Composition<N, E>, E> Cooperation<N, X, E> {
    pub fn new(assembly: X) -> Self {
        Self {
            assembly,
            stores: BTreeMap::new(),
            shared: None,
            global: None,
            bindings: BTreeMap::new(),
            readonly: BTreeMap::new(),
            proof: None,
            structure: None,
            dormant: BTreeMap::new(),
            owner_epochs: BTreeMap::new(),
            scope_owners: BTreeMap::new(),
            owner_points: BTreeMap::new(),
            deferred: BTreeMap::new(),
            next_context: 2,
            failure: None,
            interrupted: false,
        }
    }

    /// Retain the typed error until the outer boundary discards any partial DTO.
    pub fn stop(&mut self, error: CooperationError<E>, budget: &WorkBudget) -> MachineError {
        if let Err(cancelled) = MachineError::check_cancelled() {
            if self.failure.is_none() {
                self.failure = Some(CooperationError::Cancelled);
            }
            return cancelled;
        }
        let resource = budget.exhausted()
            || match &error {
                CooperationError::Protocol(error) => resource_stop(error),
                CooperationError::Initialization(public::InitializationError::Kernel(error)) => {
                    resource_stop(error)
                }
                CooperationError::Initialization(public::InitializationError::Materialization(
                    MachineError::Work,
                )) => true,
                CooperationError::Initialization(public::InitializationError::Input(
                    analysis_kernel::PublishError::Kernel(error),
                )) => resource_stop(error),
                CooperationError::Initialization(public::InitializationError::Feedback(
                    analysis_kernel::PublishError::Kernel(error),
                )) => resource_stop(error),
                CooperationError::Refinement(public::RefinementError::Stopped(
                    MachineError::Work,
                )) => true,
                _ => false,
            };
        if resource {
            self.interrupted = true;
            return MachineError::work_failure();
        }
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        MachineError::Shape
    }
    pub fn take_failure(&mut self) -> Option<CooperationError<E>> {
        self.failure.take()
    }

    pub(in crate::analysis) fn subject(&self, point: Point) -> Option<Subject> {
        self.bindings
            .get(&point)
            .map(|binding| binding.input.subject())
    }

    pub fn evaluate(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        task: Arc<Task<<X as InputTasks>::Task>>,
        token: RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        self.evaluate_task(runtime, task, token, budget, None)
    }

    fn evaluate_task(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        task: Arc<Task<<X as InputTasks>::Task>>,
        mut token: RunningJob,
        budget: &mut WorkBudget,
        draining: Option<JobId>,
    ) -> Result<(), CooperationError<E>> {
        if let TaskKind::Seed(subject) = &task.0 {
            return self.evaluate_seed(runtime, *subject, token, budget, draining);
        }
        let TaskKind::Extension(task) = &task.0 else {
            return Err(CooperationError::Protocol(KernelError::InvalidJob));
        };
        let mut meter = MachineMeter(budget);
        analysis_kernel::Meter::reserve(&mut meter, ((usize::BITS as usize) + 1) * 64)
            .map_err(KernelError::Stopped)?;
        let mut run = runtime.kernel().resume(&mut token, &mut meter)?;
        let scope = run.scope();
        let local = self
            .stores
            .get_mut(&scope)
            .ok_or(KernelError::InvalidHandle)?;
        let mut ports = Ports {
            shared: self.shared.as_mut().ok_or(KernelError::InvalidHandle)?,
            local,
            global: self.global.as_mut().ok_or(KernelError::InvalidHandle)?,
        };
        let mut cx = Context::new(&mut run, &mut ports);
        let result = self.assembly.evaluate(task, &mut cx);
        // A global ascending publication may invalidate this very callback.
        // Root stop conditions still win; stale local errors/results are handed
        // back to the already-woken interpreter owner for fresh seeding.
        let stale = {
            let (kernel, meter) = run.parts();
            analysis_kernel::Meter::checkpoint(meter).map_err(KernelError::Stopped)?;
            kernel.scope_stale(scope, meter)?
        };
        if stale {
            return Ok(());
        }
        run.finish(result.map_err(CooperationError::Extension)?)?;
        self.wake_ready(runtime, scope, meter.0, draining)
    }

    fn wake_ready(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        scope: analysis_kernel::Scope,
        budget: &mut WorkBudget,
        draining: Option<JobId>,
    ) -> Result<(), CooperationError<E>> {
        let mut meter = MachineMeter(budget);
        let status = runtime.view().closure_in_metered(scope, &mut meter)?;
        if clients_ready(status) {
            runtime.notify_observation(meter.0)?;
        }
        if clients_ready(status)
            && let Some(owner) = self.scope_owners.get(&scope).copied()
            && self.deferred.contains_key(&owner)
            && draining != Some(owner)
        {
            // Ready equivalent Top still completes a waiting native transfer.
            match runtime.kernel().retry(owner, &mut meter) {
                Ok(()) | Err(KernelError::AlreadyRunning) => {
                    // Nested drains can complete a suspended ancestor's clients.
                    // Its token and any semantic dirty wake remain with Kernel.
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn drain(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
        consumer: JobId,
    ) -> Result<(), CooperationError<E>> {
        while let Some((task, token)) = runtime.next_external(budget)? {
            self.evaluate_task(runtime, task, token, budget, Some(consumer))?;
        }
        Ok(())
    }

    /// Entry publication can wake current World callbacks as well as Body
    /// clients. Execute those callbacks before checking native relative closure.
    /// Native mandatory dependencies retain their real Deferred obligations;
    /// a reader of completion cannot itself become a prerequisite for it.
    pub(in crate::analysis) fn drain_observers(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        producer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        let owner = self.consumer_job(runtime, producer, budget)?;
        self.drain(runtime, budget, owner)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "fixed input identity, original semantics and suspended consumer"
    )]
    pub fn input<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        point: Point,
        anchor: Version,
        phase: u8,
        secondary: Option<Version>,
        parent: Option<Subject>,
        payload: EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
        observe_anchor: impl FnOnce(
            &mut analysis_kernel::Run<'_, Task<<X as InputTasks>::Task>, MachineMeter<'_>>,
        ) -> Result<(), KernelError<MachineError>>,
    ) -> Result<LocalInput<N>, CooperationError<E>> {
        // Disabled extensions bypass this owned local-view allocation at every
        // caller, preserving the default execution path and its work tariff.
        debug_assert!(X::ENABLED);
        if point.kind == PointKind::ProcedureEntry {
            return Err(KernelError::InvalidJob.into());
        }
        // Point/scope keys have fixed width; reserve a usize-sized tree-height
        // envelope before either route lookup or retained generation metadata.
        analysis_kernel::Meter::reserve(
            &mut MachineMeter(budget),
            ((usize::BITS as usize) + 1) * 64,
        )
        .map_err(KernelError::Stopped)?;
        self.ensure_shared(runtime, consumer, budget)?;
        if self.global.is_none() {
            self.global = Some(
                self.assembly
                    .create_shared(runtime.kernel(), &mut MachineMeter(budget))?,
            );
        }
        let consumer_job = self.consumer_job(runtime, consumer, budget)?;
        if let Some(parent) = parent
            && !self.receipt_valid(parent, runtime, budget)?
        {
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        }
        let stamp = Stamp {
            anchor,
            phase,
            secondary,
            parent,
        };
        let stale = if let Some(binding) = self.bindings.get(&point) {
            runtime
                .kernel()
                .generation_stale(&binding.entry, &mut MachineMeter(budget))?
        } else {
            false
        };
        if stale
            || self
                .bindings
                .get(&point)
                .is_none_or(|binding| binding.stamp != stamp)
        {
            if point.kind == PointKind::Body {
                // Replacement is a changed proof basis even if its previous
                // Input was still Pending and therefore adopted no head.
                self.structure_changed(runtime, consumer, budget)?;
            }
            let previous = self
                .bindings
                .get(&point)
                .map(|binding| binding.input.subject().scope);
            let key = if previous.is_none()
                && let Some(dormant) = self.dormant.get(&point)
            {
                ScopeKey {
                    context: dormant.context,
                    generation: dormant.next_generation,
                    policy: 0,
                }
            } else {
                self.next_scope_key(previous)?
            };
            // The caller already paid the detached payload; retain/release and
            // route construction are reserved before installing the snapshot.
            let work = payload
                .work_size()
                .saturating_mul(2)
                .saturating_add(self.assembly.registration_work())
                .saturating_add(16);
            analysis_kernel::Meter::reserve(&mut MachineMeter(budget), work)
                .map_err(KernelError::Stopped)?;
            let scope = runtime.kernel().scope(key, &mut MachineMeter(budget))?;
            let subject = Subject { point, scope };
            // A dependency-only reseed retains the exact native snapshot and
            // symbolic roots. It never carries old local guarantees forward.
            let payload = if let Some(binding) = self.bindings.get(&point)
                && binding.stamp == stamp
            {
                drop(payload);
                binding.input.payload().clone()
            } else {
                payload
            };
            let input = Input::new(subject, payload).in_universe(runtime.world);
            let local = self
                .assembly
                .create(runtime.kernel(), &mut MachineMeter(budget))?;
            let seed = runtime.kernel().register(
                scope,
                Task(TaskKind::Seed(subject)),
                &mut MachineMeter(budget),
            )?;
            self.stores.insert(scope, local);
            for task in self.assembly.tasks(subject) {
                runtime.kernel().register(
                    scope,
                    Task(TaskKind::Extension(task)),
                    &mut MachineMeter(budget),
                )?;
            }
            runtime
                .kernel()
                .seal_scope(scope, &mut MachineMeter(budget))?;
            // Register and seal before selecting; register the consumer's
            // selector dependency before running any external provider.
            let owner = match point.kind {
                PointKind::World => runtime.world,
                PointKind::Body => runtime.relative,
                PointKind::ProcedureEntry => return Err(KernelError::InvalidJob.into()),
            };
            let mut retired = self
                .bindings
                .get(&point)
                .map(|binding| binding.input.subject().scope);
            let entry = if let Some(binding) = self.bindings.get_mut(&point) {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().resume(consumer, &mut meter)?;
                run.replace_generation(&mut binding.entry, scope)?;
                *consumer = run.suspend();
                binding.entry.clone()
            } else if let Some(dormant) = self.dormant.get_mut(&point) {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().resume(consumer, &mut meter)?;
                let selected = run.current_generation(&dormant.entry)?;
                retired = selected.value().copied();
                run.replace_generation(&mut dormant.entry, scope)?;
                *consumer = run.suspend();
                dormant.entry.clone()
            } else {
                runtime
                    .kernel()
                    .generation_entry(owner, scope, &mut MachineMeter(budget))?
            };
            {
                // Install the entry before adopting native assumptions; a slot
                // publication can now stale this selected generation immediately.
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().begin(seed, &mut meter)?;
                observe_anchor(&mut run)?;
                let parent_ready = if let Some(parent) = parent {
                    let observed = run.query(
                        &mut self
                            .shared
                            .as_mut()
                            .ok_or(KernelError::InvalidHandle)?
                            .inputs,
                        analysis_kernel::Scoped::new(parent.scope, ()),
                    )?;
                    matches!(observed.state, Availability::Value(_))
                } else {
                    true
                };
                if parent_ready {
                    let mut ports = Ports {
                        shared: self.shared.as_mut().ok_or(KernelError::InvalidHandle)?,
                        local: self
                            .stores
                            .get_mut(&scope)
                            .ok_or(KernelError::InvalidHandle)?,
                        global: self.global.as_mut().ok_or(KernelError::InvalidHandle)?,
                    };
                    public::initialize(&mut run, &mut ports, input.clone())?;
                }
                // Unavailable adopted parent leaves Inputs genuinely Pending;
                // no scoped client can consume a falsely current old payload.
                run.finish(LocalClosure::Stable)?;
            }
            if let Some(retired) = retired {
                self.reclaim_retired(point, retired, runtime, budget)?;
            }
            self.publish_point(point, &entry, runtime, consumer, budget)?;
            self.dormant.remove(&point);
            self.scope_owners.insert(scope, consumer_job);
            if let Some(previous) = self.bindings.get(&point).map(|binding| binding.consumer)
                && previous != consumer_job
                && let Some(points) = self.owner_points.get_mut(&previous)
            {
                points.remove(&point);
            }
            self.owner_points
                .entry(consumer_job)
                .or_default()
                .insert(point);
            self.bindings.insert(
                point,
                Binding {
                    stamp,
                    input,
                    entry,
                    producer: None,
                    consumer: consumer_job,
                },
            );
        } else {
            // The materialized root may be fresh, but one generation always
            // uses its original native snapshot and associated symbolic roots.
            drop(payload);
        }
        if point.namespace == 2 {
            let job = consumer_job;
            let epoch = *self.owner_epochs.get(&job).ok_or(KernelError::InvalidJob)?;
            self.bindings
                .get_mut(&point)
                .ok_or(KernelError::InvalidHandle)?
                .producer = Some((job, epoch));
        }
        {
            let binding = self
                .bindings
                .get(&point)
                .ok_or(KernelError::InvalidHandle)?;
            let mut meter = MachineMeter(budget);
            let mut run = runtime.kernel().resume(consumer, &mut meter)?;
            run.current_generation(&binding.entry)?;
            *consumer = run.suspend();
        }
        self.drain(runtime, budget, consumer_job)?;
        let binding = self
            .bindings
            .get(&point)
            .ok_or(KernelError::InvalidHandle)?;
        // Known required producer work cannot be replaced by seeded Top.
        // A native deferred transfer owns an explicit obligation in this SAME
        // Kernel until it has really executed under a ready current generation.
        let stale = runtime
            .kernel()
            .generation_stale(&binding.entry, &mut MachineMeter(budget))?;
        let current = {
            let mut meter = MachineMeter(budget);
            let mut run = runtime.kernel().resume(consumer, &mut meter)?;
            let current = run.current_generation(&binding.entry)?;
            *consumer = run.suspend();
            current
        };
        let Availability::Value(scope) = current.state else {
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        };
        if stale {
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        }
        let status = runtime
            .view()
            .closure_in_metered(scope, &mut MachineMeter(budget))?;
        if !clients_ready(status) {
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        }
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(consumer, &mut meter)?;
        let published = run.query(
            &mut self
                .shared
                .as_mut()
                .ok_or(KernelError::InvalidHandle)?
                .inputs,
            analysis_kernel::Scoped::new(scope, ()),
        )?;
        if !matches!(published.state, Availability::Value(_)) {
            *consumer = run.suspend();
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        }
        let answer = public::read_refinements(
            &mut run,
            self.shared.as_mut().ok_or(KernelError::InvalidHandle)?,
            Subject { point, scope },
        )?;
        *consumer = run.suspend();
        let Availability::Value(facts) = answer.state else {
            self.defer_owner(consumer_job, runtime, budget)?;
            return Ok(LocalInput::Deferred);
        };
        analysis_kernel::Meter::reserve(&mut MachineMeter(budget), binding.input.work_size())
            .map_err(KernelError::Stopped)?;
        let view = binding.input.payload().clone();
        public::apply_refinements(view, &facts, domain, budget)
            .map_err(CooperationError::Refinement)
    }
}

fn clients_ready(status: analysis_kernel::Closure) -> bool {
    match status {
        analysis_kernel::Closure::Closed => true,
        // Native component completion and joint semantic obligations differ.
        // Missing required values or unfinished callbacks defer the transfer;
        // an independently reported obligation still prevents joint closure.
        analysis_kernel::Closure::Open(open) => {
            open.missing == 0
                && open.pending == 0
                && open.in_flight == 0
                && open.interrupted == 0
                && open.unsealed == 0
        }
    }
}

fn resource_stop(error: &KernelError<MachineError>) -> bool {
    matches!(
        error,
        KernelError::CostOverflow | KernelError::Stopped(MachineError::Work)
    )
}

mod lifecycle;
mod observations;
mod proof;
mod readonly;
mod seed;

#[cfg(test)]
mod tests;
