//! Policy/value separation and static composition.
//!
//! Joins cover both inputs and obey their advertised semantic lattice laws.
//! Refinements exchange entailed guarantees within one immutable input scope;
//! they never serve as global path-state joins. Equality is semantic policy
//! equality: representation churn alone must not keep a fixed point alive.

use crate::Either;

pub use crate::admission::{AdmissionPolicy, Prepared};

/// Ascending merge of alternative behaviors, without local reduction/widening.
pub trait JoinPolicy {
    /// Pure semantic carrier; policy configuration is kept separately.
    type Value;
    /// Typed semantic refusal, distinct from resource exhaustion.
    type Error;
    /// Allocation-free quotation for construction, traversal and comparison.
    /// Quoting must not deeply clone or enumerate the operands before payment.
    /// Include cloning either operand: [`Lift`] uses the bound for Bottom joins.
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize;
    /// Cover both inputs; lawful joins are associative, commutative and idempotent.
    fn join(&self, old: &Self::Value, incoming: &Self::Value) -> Result<Self::Value, Self::Error>;
    /// No semantically observable change under this policy.
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool;
}

/// Descending exchange of guarantees for a fixed semantic input generation.
pub trait RefinePolicy {
    /// Local guarantee carrier, independent of the global state representation.
    type Value;
    /// Typed refusal or proved contradiction; resource refusal is not bottom.
    type Error;
    /// Allocation-free quotation including refinement construction/comparison.
    /// Quoting must not deeply clone or enumerate the operands before payment.
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize;
    /// Retain all represented executions while importing entailed guarantees.
    fn refine(&self, old: &Self::Value, incoming: &Self::Value)
    -> Result<Self::Value, Self::Error>;
    /// Semantic equality for detecting actual local strengthening.
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool;
}

/// Explicitly choose ascending publication for a channel.
#[derive(Clone, Copy, Debug)]
pub struct Ascending<P>(pub P);
/// Explicitly choose generation-local descending publication for a channel.
#[derive(Clone, Copy, Debug)]
pub struct Descending<P>(pub P);

/// Common implementation surface of explicitly directed channel policies.
///
/// Prefer [`Ascending`] and [`Descending`]. Custom implementations must preserve
/// the indicated order and include all combine/equality work in `work`.
pub trait PublicationPolicy {
    /// Semantic carrier stored by the channel.
    type Value;
    /// Typed semantic refusal.
    type Error;
    /// Whether a registered generation-local reduction is required.
    const DESCENDING: bool;
    /// Upper bound for constructing and comparing a combined value.
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize;
    /// Prepare a complete value without mutating either input.
    fn combine(
        &self,
        old: &Self::Value,
        incoming: &Self::Value,
    ) -> Result<Self::Value, Self::Error>;
    /// Semantic equality for publication versions.
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool;
}
impl<P: JoinPolicy> PublicationPolicy for Ascending<P> {
    type Value = P::Value;
    type Error = P::Error;
    const DESCENDING: bool = false;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        self.0.work(old, incoming)
    }
    fn combine(
        &self,
        old: &Self::Value,
        incoming: &Self::Value,
    ) -> Result<Self::Value, Self::Error> {
        self.0.join(old, incoming)
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        self.0.equivalent(left, right)
    }
}
impl<P: RefinePolicy> PublicationPolicy for Descending<P> {
    type Value = P::Value;
    type Error = P::Error;
    const DESCENDING: bool = true;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        self.0.work(old, incoming)
    }
    fn combine(
        &self,
        old: &Self::Value,
        incoming: &Self::Value,
    ) -> Result<Self::Value, Self::Error> {
        self.0.refine(old, incoming)
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        self.0.equivalent(left, right)
    }
}

/// Static Cartesian policy composition; cooperation is separately scheduled.
#[derive(Clone, Copy, Debug)]
pub struct Product<A, B>(pub A, pub B);
impl<A: JoinPolicy, B: JoinPolicy> JoinPolicy for Product<A, B> {
    type Value = (A::Value, B::Value);
    type Error = Either<A::Error, B::Error>;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        self.0
            .work(&old.0, &incoming.0)
            .saturating_add(self.1.work(&old.1, &incoming.1))
    }
    fn join(&self, old: &Self::Value, incoming: &Self::Value) -> Result<Self::Value, Self::Error> {
        Ok((
            self.0.join(&old.0, &incoming.0).map_err(Either::Left)?,
            self.1.join(&old.1, &incoming.1).map_err(Either::Right)?,
        ))
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        self.0.equivalent(&left.0, &right.0) && self.1.equivalent(&left.1, &right.1)
    }
}
impl<A: RefinePolicy, B: RefinePolicy> RefinePolicy for Product<A, B> {
    type Value = (A::Value, B::Value);
    type Error = Either<A::Error, B::Error>;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        self.0
            .work(&old.0, &incoming.0)
            .saturating_add(self.1.work(&old.1, &incoming.1))
    }
    fn refine(
        &self,
        old: &Self::Value,
        incoming: &Self::Value,
    ) -> Result<Self::Value, Self::Error> {
        Ok((
            self.0.refine(&old.0, &incoming.0).map_err(Either::Left)?,
            self.1.refine(&old.1, &incoming.1).map_err(Either::Right)?,
        ))
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        self.0.equivalent(&left.0, &right.0) && self.1.equivalent(&left.1, &right.1)
    }
}

/// Explicit unreachable bottom; pending computation is represented elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reachable<V> {
    /// No represented execution contribution.
    Bottom,
    /// A possibly imprecise reachable upper approximation.
    Value(V),
}
/// Lift a join policy with explicit unreachable bottom.
#[derive(Clone, Copy, Debug)]
pub struct Lift<P>(pub P);
impl<P: JoinPolicy> JoinPolicy for Lift<P>
where
    P::Value: Clone,
{
    type Value = Reachable<P::Value>;
    type Error = P::Error;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        match (old, incoming) {
            (Reachable::Value(a), Reachable::Value(b)) => self.0.work(a, b).saturating_add(1),
            (Reachable::Value(v), _) | (_, Reachable::Value(v)) => {
                self.0.work(v, v).saturating_add(1)
            }
            _ => 1,
        }
    }
    fn join(&self, old: &Self::Value, incoming: &Self::Value) -> Result<Self::Value, Self::Error> {
        Ok(match (old, incoming) {
            (Reachable::Value(a), Reachable::Value(b)) => Reachable::Value(self.0.join(a, b)?),
            (Reachable::Value(v), _) | (_, Reachable::Value(v)) => Reachable::Value(v.clone()),
            _ => Reachable::Bottom,
        })
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        match (left, right) {
            (Reachable::Bottom, Reachable::Bottom) => true,
            (Reachable::Value(a), Reachable::Value(b)) => self.0.equivalent(a, b),
            _ => false,
        }
    }
}
