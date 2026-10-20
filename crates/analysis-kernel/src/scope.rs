//! Explicit analysis, context, input-generation and policy identities.

use crate::IdentityExhausted;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Process-unique namespace; no handle may cross kernel epochs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(pub(crate) u64);

impl Epoch {
    pub(crate) fn fresh() -> Result<Self, IdentityExhausted> {
        NEXT_EPOCH
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(Self)
            .map_err(|_| IdentityExhausted)
    }
}

/// Adapter-defined identities; numeric equality alone never establishes a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeKey {
    /// Program location/context, including relevant state/checkpoint identities.
    pub context: u64,
    /// Immutable semantic input version; growth opens a fresh generation.
    pub generation: u64,
    /// Frozen representation and rule policy identity.
    pub policy: u64,
}

/// An interned input generation in one kernel epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub(crate) epoch: Epoch,
    pub(crate) birth: Epoch,
    pub(crate) index: usize,
    pub(crate) key: ScopeKey,
}

impl Scope {
    /// Owning kernel epoch.
    pub fn epoch(self) -> Epoch {
        self.epoch
    }
    /// Context, input generation and policy identity supplied by the adapter.
    pub fn key(self) -> ScopeKey {
        self.key
    }
}

/// A query key explicitly bound to an immutable input generation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scoped<K> {
    /// Identity of the semantic assumptions supporting this query.
    pub scope: Scope,
    /// Capability-specific query key.
    pub key: K,
}

impl<K> Scoped<K> {
    /// Bind a query to a previously admitted scope.
    pub fn new(scope: Scope, key: K) -> Self {
        Self { scope, key }
    }
}

/// Semantic publication version, independent of the input-generation number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub(crate) u64);

impl Version {
    /// Stable counter for recorded dependency stamps and certificate guards.
    pub fn get(self) -> u64 {
        self.0
    }
    pub(crate) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}
