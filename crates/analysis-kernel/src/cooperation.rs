//! Static independent analyzers, explicit capability routes and joint solving.

use crate::{
    Accepts, Answer, Capability, Change, Channel, Charges, Discover, Kernel, KernelError,
    LocalClosure, Meter, PublishError, Read, ReadGeneration, Run, Scope, Scoped, SolveError,
    SolveReport, algebra::PublicationPolicy,
};
use std::{fmt, marker::PhantomData};

/// Select the first provider in a static store list.
pub struct Here;
/// Select a provider in the tail, with an explicit nested path.
/// The marker follows the tail path's ownership and auto traits.
pub struct There<P>(PhantomData<P>);
/// End of a statically composed provider list.
#[derive(Default)]
pub struct Nil;
/// Static provider list, allowing different carriers and duplicate query families.
pub struct Cons<H, T> {
    /// The first explicitly routed provider.
    pub head: H,
    /// Remaining providers.
    pub tail: T,
}
impl<H, T> Cons<H, T> {
    /// Assemble independently implemented providers without type erasure.
    pub fn new(head: H, tail: T) -> Self {
        Self { head, tail }
    }
}
/// Explicit provider selection; no overlapping type-search implementations.
pub trait Select<Path> {
    /// Provider at the declared path.
    type Output;
    /// Borrow the selected typed provider.
    fn select(&self) -> &Self::Output;
    /// Mutably borrow the selected typed provider.
    fn select_mut(&mut self) -> &mut Self::Output;
}
impl<H, T> Select<Here> for Cons<H, T> {
    type Output = H;
    fn select(&self) -> &H {
        &self.head
    }
    fn select_mut(&mut self) -> &mut H {
        &mut self.head
    }
}
impl<H, T: Select<P>, P> Select<There<P>> for Cons<H, T> {
    type Output = T::Output;
    fn select(&self) -> &Self::Output {
        self.tail.select()
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        self.tail.select_mut()
    }
}

/// A statically typed sum for task and error composition, not a shared fact enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Either<A, B> {
    /// Task/error owned by the first independent component.
    Left(A),
    /// Task/error owned by the second independent component.
    Right(B),
}
impl<A: fmt::Display, B: fmt::Display> fmt::Display for Either<A, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left(v) => v.fmt(f),
            Self::Right(v) => v.fmt(f),
        }
    }
}
impl<A: std::error::Error + 'static, B: std::error::Error + 'static> std::error::Error
    for Either<A, B>
{
}

/// A capability context borrowing one live run and statically composed providers.
pub struct Context<'r, 'k, J, M: Meter, S> {
    run: &'r mut Run<'k, J, M>,
    stores: &'r mut S,
}
impl<'r, 'k, J, M: Meter, S> Context<'r, 'k, J, M, S> {
    /// Assemble the same tracked primitives for a production or scalar job.
    pub fn new(run: &'r mut Run<'k, J, M>, stores: &'r mut S) -> Self {
        Self { run, stores }
    }
    /// Access shared tracked operations and the original root ledger.
    pub fn run(&mut self) -> &mut Run<'k, J, M> {
        self.run
    }
}
impl<J, M: Meter, S> ReadGeneration for Context<'_, '_, J, M, S> {
    type Error = KernelError<M::Stop>;
    fn current_generation(
        &mut self,
        entry: &crate::GenerationEntry,
    ) -> Result<Answer<Scope>, Self::Error> {
        self.run.current_generation(entry)
    }
}
impl<J, M, S, Q, P, Path> Read<Q, Path> for Context<'_, '_, J, M, S>
where
    M: Meter,
    Q: Capability,
    P: PublicationPolicy<Value = Q::Value>,
    S: Select<Path, Output = Channel<Q, P>>,
{
    type Error = KernelError<M::Stop>;
    fn query(&mut self, key: Scoped<Q::Key>) -> Result<Answer<Q::Value>, Self::Error> {
        self.run.query(self.stores.select_mut(), key)
    }
}
impl<J, M, S, Q, P, Path> Discover<Q, Path> for Context<'_, '_, J, M, S>
where
    M: Meter,
    Q: Capability,
    P: PublicationPolicy<Value = Q::Value>,
    S: Select<Path, Output = Channel<Q, P>>,
{
    type Error = KernelError<M::Stop>;
    fn discover(&mut self, key: Scoped<Q::Key>) -> Result<Answer<Q::Value>, Self::Error> {
        self.run.query_discovery(self.stores.select_mut(), key)
    }
}
impl<J, M, S, Q, P, Path> Accepts<Q, Path> for Context<'_, '_, J, M, S>
where
    M: Meter,
    Q: Capability,
    P: PublicationPolicy<Value = Q::Value>,
    S: Select<Path, Output = Channel<Q, P>>,
{
    type Error = PublishError<P::Error, M::Stop>;
    fn accept(&mut self, key: Scoped<Q::Key>, value: Q::Value) -> Result<Change, Self::Error> {
        self.run.publish(self.stores.select_mut(), key, value)
    }
}
impl<J, M: Meter, S> Charges for Context<'_, '_, J, M, S> {
    type Error = KernelError<M::Stop>;
    fn charge(&mut self, work: usize) -> Result<(), Self::Error> {
        self.run.reserve(work)
    }
}

/// Independent semantic analysis written against only its declared capabilities.
pub trait Analyzer<Cx> {
    /// Opaque component-specific task; no fixed IR is required.
    type Task;
    /// Typed component failure.
    type Error;
    /// Publish sound consequences and report local completion/optional imprecision.
    fn evaluate(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error>;
}
/// Independent descending rule whose soundness contract is explicitly registered.
///
/// All exported guarantees must be entailed by this one immutable input scope.
/// Refinement preserves its joint concretization; only a valid typed contradiction
/// may eliminate behavior. Optional capacity refusal must retain a coarse answer.
///
/// An explicitly registered initialization rule is also the only authority for a
/// first descending write. Ordinary analyzers cannot bypass this gate on Pending.
/// Upstream growth/replacement changes the concrete-input universe: create and
/// seed a fresh scope, then replace its stable generation entry. Reawakening an
/// already narrowed old generation cannot safely represent the enlarged input.
pub trait Reduction<Cx> {
    /// Rule-specific task, independent of other analyzers' task representations.
    type Task;
    /// Typed policy/refinement error.
    type Error;
    /// Refine through typed local capabilities, using tracked queries.
    fn reduce(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error>;
}

mod registration {
    pub trait Sealed {
        fn set_registered(&mut self, active: bool) -> bool;
    }
}
/// A kernel-owned context supporting explicitly registered descending rules.
/// Sealed so registration cannot silently disappear in a custom facade.
pub trait ReductionContext: registration::Sealed {}
impl<J, M: Meter, S> registration::Sealed for Context<'_, '_, J, M, S> {
    fn set_registered(&mut self, active: bool) -> bool {
        self.run.set_reduction(active)
    }
}
impl<J, M: Meter, S> ReductionContext for Context<'_, '_, J, M, S> {}

/// Explicit static registration of a trusted descending semantic rule.
pub struct Registered<R> {
    rule: R,
}
impl<R> Registered<R> {
    /// Register a rule after accepting its documented scoped soundness contract.
    pub fn new(rule: R) -> Self {
        Self { rule }
    }
    /// Inspect retained rule diagnostics without exposing mutable policy state.
    pub fn rule(&self) -> &R {
        &self.rule
    }
}
impl<Cx: ReductionContext, R: Reduction<Cx>> Analyzer<Cx> for Registered<R> {
    type Task = R::Task;
    type Error = R::Error;
    fn evaluate(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let previous = registration::Sealed::set_registered(cx, true);
        let result = self.rule.reduce(task, cx);
        registration::Sealed::set_registered(cx, previous);
        result
    }
}

/// Compose independent analyzers/reductions without a universal shared enum.
pub struct Pair<A, B> {
    /// First independent implementation.
    pub left: A,
    /// Second independent implementation.
    pub right: B,
}
impl<A, B> Pair<A, B> {
    /// Assemble components whose tasks/errors are kept statically distinct.
    pub fn new(left: A, right: B) -> Self {
        Self { left, right }
    }
}
impl<Cx, A: Analyzer<Cx>, B: Analyzer<Cx>> Analyzer<Cx> for Pair<A, B> {
    type Task = Either<A::Task, B::Task>;
    type Error = Either<A::Error, B::Error>;
    fn evaluate(&mut self, task: &Self::Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        match task {
            Either::Left(task) => self.left.evaluate(task, cx).map_err(Either::Left),
            Either::Right(task) => self.right.evaluate(task, cx).map_err(Either::Right),
        }
    }
}

/// Drain the same tracked scheduler used by production adapters, within a step cap.
///
/// A local precision limit can be accepted with a sound coarse answer; pending
/// required jobs, missing providers and explicit semantic obligations remain in
/// the returned closure. Errors retain the currently admitted/in-flight job.
/// A callback invalidating its own selected scope returns the recoverable
/// `SolveError::Kernel(KernelError::StaleGeneration)` completion boundary. Its
/// stable owner stays scheduled and its stale scope keeps closure Open. Actual
/// meter/cancel failures and analyzer errors are preserved; the caller can reseed
/// and replace the generation before continuing this solver.
/// The final report traversal is reserved on the same meter even with zero steps
/// or an empty ready queue; cancellation/resource refusal cannot bypass it.
pub fn solve<J, A, S, M, E>(
    kernel: &mut Kernel<J>,
    analyzer: &mut A,
    stores: &mut S,
    meter: &mut M,
    max_steps: usize,
) -> Result<SolveReport, SolveError<E, M::Stop>>
where
    M: Meter,
    for<'r, 'k> A: Analyzer<Context<'r, 'k, J, M, S>, Task = J, Error = E>,
{
    let mut steps = 0;
    while steps < max_steps {
        let Some(id) = kernel.next(meter).map_err(SolveError::Kernel)? else {
            break;
        };
        let task = kernel.task_handle(id, meter).map_err(SolveError::Kernel)?;
        let mut run = kernel.begin(id, meter).map_err(SolveError::Kernel)?;
        let local = {
            let mut cx = Context::new(&mut run, stores);
            analyzer
                .evaluate(&task, &mut cx)
                .map_err(SolveError::Analyzer)?
        };
        run.finish(local).map_err(SolveError::Kernel)?;
        steps += 1;
    }
    kernel
        .solve_report(meter, steps)
        .map_err(SolveError::Kernel)
}
