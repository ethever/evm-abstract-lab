//! Typed query families, explicit provider routes and directed publication.

use crate::{Scope, Scoped, Version, algebra::PublicationPolicy, kernel::FactId};
use std::collections::BTreeMap;

/// A semantic query family; it does not expose a provider's representation.
pub trait Capability {
    /// Query identity within one semantic scope.
    type Key: Clone + Ord;
    /// Typed answer, independent of other families' carriers.
    type Value: Clone;
    /// Allocation-free conservative quote for key clone/comparison and drop.
    fn key_work(key: &Self::Key) -> usize;
    /// Allocation-free conservative quote for answer clone/traversal work.
    fn value_work(value: &Self::Value) -> usize;
    /// Allocation-free conservative quote for destroying a retained answer.
    /// Include traversal/deallocation of the entire possibly last-owned payload.
    /// Arc/shared-handle carriers whose clone quote covers only their handle must
    /// override this method; reclamation may release the last deep payload owner.
    fn drop_work(value: &Self::Value) -> usize {
        Self::value_work(value)
    }
}

/// Computation pending, explicit absence and semantic values are different states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability<V> {
    /// No provider has yet resolved this answer; it keeps a read obligation open.
    Pending,
    /// No contribution is currently published on this route. This is not a
    /// permanent negative proof or a semantic Bottom; later values still wake
    /// readers. One provider cannot use it to erase another provider's value.
    Absent,
    /// A semantic answer, possibly itself a top or bottom approximation.
    Value(V),
}
/// A sound typed observation and the exact identity supporting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer<V> {
    /// Availability is not a feasibility or execution-completion assertion.
    pub state: Availability<V>,
    /// This fact's publication/coverage-refresh counter. Producer invalidation
    /// can make Availability Pending without changing this counter. It alone is
    /// not a validity certificate: consume values through tracked guard checks.
    pub version: Version,
    /// Immutable assumptions supporting this observation.
    pub scope: Scope,
}
impl<V> Answer<V> {
    /// Borrow an already published value without treating pending/absence as Top.
    pub fn value(&self) -> Option<&V> {
        match &self.state {
            Availability::Value(value) => Some(value),
            _ => None,
        }
    }
}
/// Whether a publication changed semantic information or declared coverage
/// availability, rather than only representation. Restoring a known producer's
/// coverage wakes readers without seeding numerical-growth invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Repeated or equivalent publication; readers are not rescheduled.
    Unchanged,
    /// A new answer/coverage version was installed and dependent readers woke.
    Changed,
}

pub(crate) struct Cell<V> {
    pub(crate) id: FactId,
    pub(crate) state: Availability<V>,
}

pub(crate) struct ScopeCells<K, V> {
    pub(crate) directory: FactId,
    pub(crate) cells: BTreeMap<K, Cell<V>>,
}

/// One explicitly routed provider/aggregation table with a declared update order.
///
/// Multiple instances of the same query family are distinct routes. There is no
/// ambient type-based provider search. Values remain here; kernel metadata never
/// erases them. Creating a channel is metered by [`crate::Kernel::channel`].
pub struct Channel<Q: Capability, P: PublicationPolicy<Value = Q::Value>> {
    pub(crate) epoch: crate::Epoch,
    pub(crate) directory: FactId,
    pub(crate) scopes: BTreeMap<Scope, ScopeCells<Q::Key, Q::Value>>,
    pub(crate) len: usize,
    pub(crate) policy: P,
}
impl<Q: Capability, P: PublicationPolicy<Value = Q::Value>> Channel<Q, P> {
    /// Number of interned keys, including still-pending negative reads.
    pub fn len(&self) -> usize {
        self.len
    }
    /// Whether this route has no interned keys.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Frozen publication policy; changing policy requires a different scope/route.
    pub fn policy(&self) -> &P {
        &self.policy
    }
}

/// Declared typed read capability at an explicit static provider route.
pub trait Read<Q: Capability, Route = crate::Here> {
    /// Typed scope/meter failure of the client context.
    type Error;
    /// Record the dependency, including missing/negative reads.
    fn query(&mut self, key: Scoped<Q::Key>) -> Result<Answer<Q::Value>, Self::Error>;
}
/// Declared typed publication capability at an explicit static provider route.
pub trait Accepts<Q: Capability, Route = crate::Here> {
    /// Typed policy or infrastructure failure.
    type Error;
    /// Publish through the selected route's declared ascending/refining policy.
    fn accept(&mut self, key: Scoped<Q::Key>, value: Q::Value) -> Result<Change, Self::Error>;
}

/// Shared-ledger accounting for independently implemented semantic computations.
pub trait Charges {
    /// Original typed resource/scope failure of the client context.
    type Error;
    /// Reserve a conservative computation bound before executing a rule.
    fn charge(&mut self, work: usize) -> Result<(), Self::Error>;
}

/// Read a stable generation route without adopting its numeric scope identity as
/// a semantic assumption. A stale selection is Pending until its owner replaces it.
pub trait ReadGeneration {
    /// Typed identity/meter failure of the client context.
    type Error;
    /// Track the route even when its selected generation is stale.
    fn current_generation(
        &mut self,
        entry: &crate::GenerationEntry,
    ) -> Result<Answer<Scope>, Self::Error>;
}
