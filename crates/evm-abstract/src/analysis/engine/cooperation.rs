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
            LocalInput, Point, PointKind, Ports, Subject,
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
    Analyzer, Availability, Context, GenerationEntry, KernelError, LocalClosure, RunningJob,
    ScopeKey, Version,
};
use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

/// A bound alias only: semantic implementations use the existing kernel traits.
pub(in crate::analysis) trait Composition<N: WordElement, E>:
    InputTasks
    + ExtraStores
    + for<'r, 'k, 'm> Analyzer<
        EvmContext<'r, 'k, 'm, N, <Self as InputTasks>::Task, <Self as ExtraStores>::Stores>,
        Task = <Self as InputTasks>::Task,
        Error = E,
    >
{
}
impl<N: WordElement, E, X> Composition<N, E> for X where
    X: InputTasks
        + ExtraStores
        + for<'r, 'k, 'm> Analyzer<
            EvmContext<'r, 'k, 'm, N, <X as InputTasks>::Task, <X as ExtraStores>::Stores>,
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
    fn create<J, M: analysis_kernel::Meter>(
        &self,
        kernel: &mut analysis_kernel::Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        self.0.create(kernel, meter)
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
}
struct Binding<N: WordElement> {
    stamp: Stamp,
    input: Input<N>,
    entry: GenerationEntry,
}

pub(in crate::analysis) struct Cooperation<N: WordElement, X: InputTasks + ExtraStores, E> {
    assembly: X,
    stores: BTreeMap<analysis_kernel::Scope, Ports<N, <X as ExtraStores>::Stores>>,
    bindings: BTreeMap<Point, Binding<N>>,
    next_context: u64,
    failure: Option<CooperationError<E>>,
    pub interrupted: bool,
    marker: PhantomData<fn() -> N>,
}

pub(in crate::analysis) type Empty = public::Extension<public::NoExtension, public::NoTasks>;

impl<N: WordElement, X: Composition<N, E>, E> Cooperation<N, X, E> {
    pub fn new(assembly: X) -> Self {
        Self {
            assembly,
            stores: BTreeMap::new(),
            bindings: BTreeMap::new(),
            next_context: 2,
            failure: None,
            interrupted: false,
            marker: PhantomData,
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

    pub fn settled(&self, runtime: &Runtime<<X as InputTasks>::Task>) -> bool {
        self.bindings.values().all(|binding| {
            runtime
                .view()
                .closure_in::<MachineError>(binding.input.subject().scope)
                == Ok(analysis_kernel::Closure::Closed)
        })
    }

    pub fn evaluate(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        task: Arc<Task<<X as InputTasks>::Task>>,
        mut token: RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        let TaskKind::Extension(task) = &task.0 else {
            return Err(CooperationError::Protocol(KernelError::InvalidJob));
        };
        let mut meter = MachineMeter(budget);
        analysis_kernel::Meter::reserve(&mut meter, ((usize::BITS as usize) + 1) * 64)
            .map_err(KernelError::Stopped)?;
        let mut run = runtime.kernel().resume(&mut token, &mut meter)?;
        let scope = run.scope();
        let ports = self
            .stores
            .get_mut(&scope)
            .ok_or(KernelError::InvalidHandle)?;
        let mut cx = Context::new(&mut run, ports);
        let closure = self
            .assembly
            .evaluate(task, &mut cx)
            .map_err(CooperationError::Extension)?;
        run.finish(closure)?;
        Ok(())
    }

    fn drain(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        budget: &mut WorkBudget,
    ) -> Result<(), CooperationError<E>> {
        while let Some((task, token)) = runtime.next_external(budget)? {
            self.evaluate(runtime, task, token, budget)?;
        }
        Ok(())
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
        payload: EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        consumer: &mut RunningJob,
        budget: &mut WorkBudget,
    ) -> Result<LocalInput<N>, CooperationError<E>> {
        // Disabled extensions bypass this owned local-view allocation at every
        // caller, preserving the default execution path and its work tariff.
        debug_assert!(X::ENABLED);
        // Point/scope keys have fixed width; reserve a usize-sized tree-height
        // envelope before either route lookup or retained generation metadata.
        analysis_kernel::Meter::reserve(
            &mut MachineMeter(budget),
            ((usize::BITS as usize) + 1) * 64,
        )
        .map_err(KernelError::Stopped)?;
        let stamp = Stamp {
            anchor,
            phase,
            secondary,
        };
        if self
            .bindings
            .get(&point)
            .is_none_or(|binding| binding.stamp != stamp)
        {
            let (context, generation) = if let Some(binding) = self.bindings.get(&point) {
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
            // The caller already paid the detached payload; retain/release and
            // route construction are reserved before installing the snapshot.
            let work = payload
                .work_size()
                .saturating_mul(2)
                .saturating_add(self.assembly.registration_work())
                .saturating_add(16);
            analysis_kernel::Meter::reserve(&mut MachineMeter(budget), work)
                .map_err(KernelError::Stopped)?;
            let scope = runtime.kernel().scope(
                ScopeKey {
                    context,
                    generation,
                    policy: 0,
                },
                &mut MachineMeter(budget),
            )?;
            let subject = Subject { point, scope };
            let input = Input::new(subject, payload);
            let mut ports =
                public::create_ports(runtime.kernel(), &mut MachineMeter(budget), &self.assembly)?;
            let seed = runtime.kernel().register(
                scope,
                Task(TaskKind::Seed(subject)),
                &mut MachineMeter(budget),
            )?;
            {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().begin(seed, &mut meter)?;
                public::initialize(&mut run, &mut ports, input.clone())?;
                run.finish(LocalClosure::Stable)?;
            }
            self.stores.insert(scope, ports);
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
            self.drain(runtime, budget)?;
            // Seeding and all initial mandatory work precede selector movement.
            let owner = match point.kind {
                PointKind::World => runtime.world,
                PointKind::Body => runtime.relative,
            };
            let entry = if let Some(binding) = self.bindings.get_mut(&point) {
                let mut meter = MachineMeter(budget);
                let mut run = runtime.kernel().resume(consumer, &mut meter)?;
                run.replace_generation(&mut binding.entry, scope)?;
                *consumer = run.suspend();
                binding.entry.clone()
            } else {
                runtime
                    .kernel()
                    .generation_entry(owner, scope, &mut MachineMeter(budget))?
            };
            self.bindings.insert(
                point,
                Binding {
                    stamp,
                    input,
                    entry,
                },
            );
        } else {
            // The materialized root may be fresh, but one generation always
            // uses its original native snapshot and associated symbolic roots.
            drop(payload);
            self.drain(runtime, budget)?;
        }
        let binding = self
            .bindings
            .get(&point)
            .ok_or(KernelError::InvalidHandle)?;
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(consumer, &mut meter)?;
        let current = run.current_generation(&binding.entry)?;
        let Availability::Value(scope) = current.state else {
            return Err(CooperationError::Unfinished(run.kernel().closure()));
        };
        let answer = public::read_refinements(
            &mut run,
            self.stores
                .get_mut(&scope)
                .ok_or(KernelError::InvalidHandle)?,
            Subject { point, scope },
        )?;
        *consumer = run.suspend();
        let Availability::Value(facts) = answer.state else {
            return Err(CooperationError::Unfinished(runtime.kernel().closure()));
        };
        analysis_kernel::Meter::reserve(&mut MachineMeter(budget), binding.input.work_size())
            .map_err(KernelError::Stopped)?;
        let view = binding.input.payload().clone();
        public::apply_refinements(view, &facts, domain, budget)
            .map_err(CooperationError::Refinement)
    }
}

fn resource_stop(error: &KernelError<MachineError>) -> bool {
    matches!(
        error,
        KernelError::CostOverflow | KernelError::Stopped(MachineError::Work)
    )
}

#[cfg(test)]
mod tests;
