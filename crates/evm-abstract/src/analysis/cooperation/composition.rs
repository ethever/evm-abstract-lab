use super::Subject;
use analysis_kernel::{Analyzer, Either, Kernel, KernelError, LocalClosure, Meter, Nil};
use std::{convert::Infallible, iter};

/// Static registration of opaque component tasks for one immutable EVM input.
/// This only routes tasks. Semantic work implements the ordinary kernel
/// Analyzer/Reduction protocols and is scheduled by its dependency graph.
pub trait InputTasks {
    /// Component-owned task representation, retained by the shared kernel.
    type Task;
    /// Whether this assembly has any external obligations.
    const ENABLED: bool;
    /// Conservative construction/iteration charge, reserved before tasks().
    /// Quoting borrows the plan and must not clone task payloads.
    fn registration_work(&self) -> usize;
    /// Seed tasks for the new universe; do not carry old generation narrowing.
    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task>;
}

/// Register one independent analyzer task for each production input generation.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerInput;

impl InputTasks for PerInput {
    type Task = Subject;
    const ENABLED: bool = true;

    fn registration_work(&self) -> usize {
        1
    }

    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Subject> {
        iter::once(subject)
    }
}

/// Task routes matching the kernel's independently scheduled Pair composition.
#[derive(Clone, Debug)]
pub struct PairedTasks<L, R> {
    /// Registration plan for the first component.
    pub left: L,
    /// Registration plan for the second component.
    pub right: R,
}

impl<L, R> PairedTasks<L, R> {
    /// Pair static task routes without hand-writing a semantic iteration loop.
    pub const fn new(left: L, right: R) -> Self {
        Self { left, right }
    }
}

impl<L: InputTasks, R: InputTasks> InputTasks for PairedTasks<L, R> {
    type Task = Either<L::Task, R::Task>;
    const ENABLED: bool = L::ENABLED || R::ENABLED;

    fn registration_work(&self) -> usize {
        self.left
            .registration_work()
            .saturating_add(self.right.registration_work())
            .saturating_add(2)
    }

    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        self.left
            .tasks(subject)
            .map(Either::Left)
            .chain(self.right.tasks(subject).map(Either::Right))
    }
}

/// Statically assembled independent components and their input task routes.
/// Neither the analyzer nor its frozen task plan needs Clone or Copy.
pub struct Extension<A, P = PerInput, F = NoStores> {
    /// An Analyzer, Registered reduction, or kernel Pair of independent rules.
    pub analyzer: A,
    /// Static task routes for each new input universe.
    pub tasks: P,
    /// Per-generation factory for independently defined typed capability stores.
    pub stores: F,
}

impl<A> Extension<A> {
    /// Attach one ordinary Analyzer/Registered reduction to each EVM input.
    pub const fn new(analyzer: A) -> Self {
        Self {
            analyzer,
            tasks: PerInput,
            stores: NoStores,
        }
    }
}

impl<A, P> Extension<A, P> {
    /// Assemble independently scheduled components with explicit typed routes.
    pub const fn with_tasks(analyzer: A, tasks: P) -> Self {
        Self {
            analyzer,
            tasks,
            stores: NoStores,
        }
    }
}

impl<A, P, F> Extension<A, P, F> {
    /// Add client-defined typed channels on the same kernel and root ledger.
    /// Their contents are recreated for each immutable input generation.
    pub fn with_stores<G>(self, stores: G) -> Extension<A, P, G> {
        Extension {
            analyzer: self.analyzer,
            tasks: self.tasks,
            stores,
        }
    }
}

impl<A, P: InputTasks, F> InputTasks for Extension<A, P, F> {
    type Task = P::Task;
    const ENABLED: bool = P::ENABLED;

    fn registration_work(&self) -> usize {
        self.tasks.registration_work()
    }

    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        self.tasks.tasks(subject)
    }
}

impl<Cx, A: Analyzer<Cx>, P, F> Analyzer<Cx> for Extension<A, P, F> {
    type Task = A::Task;
    type Error = A::Error;

    fn evaluate(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        self.analyzer.evaluate(task, cx)
    }
}

/// Assemble independent typed capability providers inside one Subject universe.
/// Factories allocate through the supplied shared kernel/meter; they must not
/// start another solve, create a root budget or copy old generation narrowing.
pub trait ExtraStores {
    /// Static Cons/channel provider list, selected by explicit kernel routes.
    type Stores;
    /// Construct fresh stores; all channel allocations use this same meter.
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>>;
}

/// No extra capability providers for the default extension assembly.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoStores;

impl ExtraStores for NoStores {
    type Stores = Nil;
    fn create<J, M: Meter>(
        &self,
        _: &mut Kernel<J>,
        _: &mut M,
    ) -> Result<Nil, KernelError<M::Stop>> {
        Ok(Nil)
    }
}

impl<A, P, F: ExtraStores> ExtraStores for Extension<A, P, F> {
    type Stores = F::Stores;
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        self.stores.create(kernel, meter)
    }
}

/// No external components. Existing APIs use this assembly in the same runtime.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoExtension;

impl<Cx> Analyzer<Cx> for NoExtension {
    type Task = Infallible;
    type Error = Infallible;

    fn evaluate(&mut self, task: &Infallible, _: &mut Cx) -> Result<LocalClosure, Infallible> {
        match *task {}
    }
}

/// Empty registration plan, preserving the default execution path and reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTasks;

impl InputTasks for NoTasks {
    type Task = Infallible;
    const ENABLED: bool = false;

    fn registration_work(&self) -> usize {
        0
    }

    fn tasks(&self, _: Subject) -> impl Iterator<Item = Infallible> {
        iter::empty()
    }
}

impl Default for Extension<NoExtension, NoTasks> {
    fn default() -> Self {
        Self::with_tasks(NoExtension, NoTasks)
    }
}
