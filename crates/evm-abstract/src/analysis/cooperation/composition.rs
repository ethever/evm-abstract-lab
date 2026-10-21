use super::{EvmContext, Subject};
use crate::domain::WordElement;
use analysis_kernel::{Analyzer, Either, Kernel, KernelError, LocalClosure, Meter, Nil, Pair};
use std::{convert::Infallible, iter};

/// Static EVM assembly bound; semantic work remains on the kernel traits.
/// Tasks match the analyzer for every context lifetime. Generation-local stores
/// and once-per-run shared stores retain their separate factories and routes.
pub trait Composition<N: WordElement, E>:
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

/// Static registration of opaque component tasks for one immutable EVM input.
/// This only routes tasks. Semantic work implements the ordinary kernel
/// Analyzer/Reduction protocols and is scheduled by its dependency graph.
pub trait InputTasks {
    /// Component-owned task representation, retained by the shared kernel.
    type Task;
    /// Whether this assembly has any external obligations.
    const ENABLED: bool;
    /// Conservative construction/iteration and eventual task-disposal charge,
    /// reserved before tasks(). Include native payload destruction, not just
    /// the kernel handle; the kernel cannot infer arbitrary Task Drop work.
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
pub type PairedTasks<L, R> = Pair<L, R>;

impl<L: InputTasks, R: InputTasks> InputTasks for Pair<L, R> {
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
pub struct Extension<A, P = PerInput, F = NoStores, G = NoStores> {
    /// An Analyzer, Registered reduction, or kernel Pair of independent rules.
    pub analyzer: A,
    /// Static task routes for each new input universe.
    pub tasks: P,
    /// Per-generation factory for independently defined typed capability stores.
    pub stores: F,
    /// Once-per-run factory for independently defined global capability stores.
    pub shared: G,
}

impl<A> Extension<A> {
    /// Attach one ordinary Analyzer/Registered reduction to each EVM input.
    pub const fn new(analyzer: A) -> Self {
        Self {
            analyzer,
            tasks: PerInput,
            stores: NoStores,
            shared: NoStores,
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
            shared: NoStores,
        }
    }
}

impl<A, P, F, G> Extension<A, P, F, G> {
    /// Add client-defined typed channels on the same kernel and root ledger.
    /// Their contents are recreated for each immutable input generation.
    pub fn with_stores<H>(self, stores: H) -> Extension<A, P, H, G> {
        Extension {
            analyzer: self.analyzer,
            tasks: self.tasks,
            stores,
            shared: self.shared,
        }
    }

    /// Add shared client channels for cross-point ascending facts. Publish under
    /// Input::universe(), use tracked queries, and retain only sound bounds.
    /// Generation-local guarantees belong in with_stores() or Refinements.
    pub fn with_shared_stores<H>(self, shared: H) -> Extension<A, P, F, H> {
        Extension {
            analyzer: self.analyzer,
            tasks: self.tasks,
            stores: self.stores,
            shared,
        }
    }
}

impl<A, P: InputTasks, F, G> InputTasks for Extension<A, P, F, G> {
    type Task = P::Task;
    const ENABLED: bool = P::ENABLED;

    fn registration_work(&self) -> usize {
        self.tasks.registration_work()
    }

    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        self.tasks.tasks(subject)
    }
}

impl<Cx, A: Analyzer<Cx>, P, F, G> Analyzer<Cx> for Extension<A, P, F, G> {
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
    /// Allocation-free upper bound for disposing this generation's client data.
    /// Include all channel values and arbitrary native client Drop work. A zero
    /// quote explicitly asserts that disposal was prepaid during construction.
    fn retirement_work(&self, stores: &Self::Stores) -> usize;
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
    fn retirement_work(&self, _: &Nil) -> usize {
        0
    }
    fn create<J, M: Meter>(
        &self,
        _: &mut Kernel<J>,
        _: &mut M,
    ) -> Result<Nil, KernelError<M::Stop>> {
        Ok(Nil)
    }
}

impl<A, P, F: ExtraStores, G> ExtraStores for Extension<A, P, F, G> {
    type Stores = F::Stores;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        self.stores.retirement_work(stores)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        self.stores.create(kernel, meter)
    }
}

/// Once-per-run typed stores shared by independently authored global analyzers.
/// This factory uses the original kernel and ledger; it does not own a solver.
pub trait SharedStores {
    /// Static channels selected with `SharedRoute<P>`.
    type Shared: analysis_kernel::ReclaimScope;
    /// Allocate the shared provider list once for the whole joint run.
    fn create_shared<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Shared, KernelError<M::Stop>>;
}
impl<A, P, F, G: ExtraStores> SharedStores for Extension<A, P, F, G>
where
    G::Stores: analysis_kernel::ReclaimScope,
{
    type Shared = G::Stores;
    fn create_shared<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Shared, KernelError<M::Stop>> {
        self.shared.create(kernel, meter)
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
