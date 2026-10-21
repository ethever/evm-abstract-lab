//! Language-independent, statically composed cooperative abstract analysis.
//!
//! [`Kernel`] tracks obligations and typed publications; semantic values stay in
//! [`Channel`]s. A query records its dependency even before any provider publishes
//! an answer. Registered descending [`Reduction`]s are scoped to one immutable
//! input generation, separately from ascending flow/summary publications.
//!
//! Publication is atomic, not a whole job: reserve policy work, prepare a value,
//! then reserve installation and wake-up work before changing shared facts.
//! Previously committed sound publications survive a later interruption, while
//! the in-flight job keeps [`Closure`] open. The language adapter remains
//! responsible for sound policies, coverage, rollback and semantic frontiers.

pub mod admission;
pub mod algebra;
pub mod capability;
pub mod cooperation;
pub mod error;
pub mod kernel;
pub mod reclaim;
pub mod scope;
pub mod status;
pub mod worklist;

pub use admission::{AdmissionPolicy, Prepared};
pub use algebra::{Ascending, Descending, JoinPolicy, Lift, Product, Reachable, RefinePolicy};
pub use capability::{
    Accepts, Answer, Availability, Capability, Change, Channel, Charges, Read, ReadGeneration,
};
pub use cooperation::{
    Analyzer, Cons, Context, Either, Here, Nil, Pair, Reduction, Registered, Select, There, solve,
};
pub use error::{IdentityExhausted, KernelError, PublishError, SolveError};
pub use kernel::{
    GenerationEntry, JobId, Kernel, KernelFork, Meter, ObligationId, Run, RunningJob, SlotMeta,
    Statistics,
};
pub use reclaim::ReclaimScope;
pub use scope::{Epoch, Scope, ScopeKey, Scoped, Version};
pub use status::{Closure, LocalClosure, Open, SolveReport};
pub use worklist::Worklist;

#[cfg(test)]
mod tests;

/// A compiling minimal client: a negative read is tracked before the first
/// publication, so the writer's self-dependency requires a second evaluation.
///
/// ```
/// use analysis_kernel::{
///     Ascending, Availability, Capability, Closure, JoinPolicy, Kernel,
///     LocalClosure, Meter, ScopeKey, Scoped,
/// };
/// use std::convert::Infallible;
///
/// struct Ledger(usize);
/// impl Meter for Ledger {
///     type Stop = Infallible;
///     fn reserve(&mut self, work: usize) -> Result<(), Infallible> {
///         self.0 += work;
///         Ok(())
///     }
///     fn checkpoint(&self) -> Result<(), Infallible> { Ok(()) }
/// }
/// struct Count;
/// impl Capability for Count {
///     type Key = ();
///     type Value = u32;
///     fn key_work(_: &()) -> usize { 1 }
///     fn value_work(_: &u32) -> usize { 1 }
/// }
/// struct Maximum;
/// impl JoinPolicy for Maximum {
///     type Value = u32;
///     type Error = Infallible;
///     fn work(&self, _: &u32, _: &u32) -> usize { 3 }
///     fn join(&self, a: &u32, b: &u32) -> Result<u32, Infallible> {
///         Ok((*a).max(*b))
///     }
///     fn equivalent(&self, a: &u32, b: &u32) -> bool { a == b }
/// }
/// let mut ledger = Ledger(0);
/// let mut kernel = Kernel::<()>::new().unwrap();
/// let scope = kernel.scope(ScopeKey {
///     context: 1, generation: 0, policy: 1,
/// }, &mut ledger).unwrap();
/// let mut counts = kernel.channel::<Count, _, _>(Ascending(Maximum), &mut ledger).unwrap();
/// let job = kernel.register(scope, (), &mut ledger).unwrap();
/// kernel.seal_scope(scope, &mut ledger).unwrap();
/// let mut run = kernel.begin(job, &mut ledger).unwrap();
/// assert_eq!(run.query(&mut counts, Scoped::new(scope, ())).unwrap().state,
///            Availability::Pending);
/// run.publish(&mut counts, Scoped::new(scope, ()), 7).unwrap();
/// run.finish(LocalClosure::Stable).unwrap();
/// assert!(matches!(kernel.closure(), Closure::Open(_)));
/// let next = kernel.next(&mut ledger).unwrap().unwrap();
/// let mut run = kernel.begin(next, &mut ledger).unwrap();
/// assert_eq!(run.query(&mut counts, Scoped::new(scope, ())).unwrap().value(), Some(&7));
/// run.finish(LocalClosure::Stable).unwrap();
/// assert_eq!(kernel.closure(), Closure::Closed);
/// assert!(ledger.0 > 0);
/// ```
pub mod example {}
