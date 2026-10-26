//! Immutable, carrier-preserving publications at semantic stage boundaries.
//!
//! A publication is a closed model of its recorded input, not a worker lifecycle
//! event. Publishers may retain a handle while optional precision continues or is
//! cancelled. A later fact restriction never changes the handle's qualification.

use super::{EvmWorldAnalysis, HybridAnalysis, control::Cancelled};
use crate::domain::WordElement;
use serde::{Serialize, Serializer};
use std::{ops::Deref, sync::Arc};

/// Shared ownership of one immutable graph, including its original input binding.
/// Cloning this handle never copies a graph or converts its numeric carrier.
#[derive(Debug)]
pub struct ModelHandle<N: WordElement>(Arc<EvmWorldAnalysis<N>>);

impl<N: WordElement> ModelHandle<N> {
    pub(in crate::analysis) fn new(model: EvmWorldAnalysis<N>) -> Self {
        Self(Arc::new(model))
    }

    /// Whether both handles retain the exact same immutable model. Equal input
    /// labels or revisions alone do not establish unchanged graph identity.
    pub fn same_model(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<N: WordElement> Clone for ModelHandle<N> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<N: WordElement> Serialize for ModelHandle<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

impl<N: WordElement> Deref for ModelHandle<N> {
    type Target = EvmWorldAnalysis<N>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Application-owned, statically dispatched semantic publication boundary.
///
/// Called once with a certified baseline before any optional Native preparation.
/// The callback may retain `baseline_handle()` and perform separately funded,
/// bounded export. Export failure must not manufacture or revoke model closure.
/// Callbacks run synchronously and should not wait for downstream consumers.
pub trait StagePublisher<N: WordElement> {
    /// Publish the baseline with its exact snapshot, revision and source guards.
    fn publish(&mut self, model: &HybridAnalysis<N>);

    /// Stop before optional preparation when the application cannot retain a
    /// useful publication. The application keeps its original typed failure;
    /// the core uses its existing control-stop channel and leaves model closure
    /// and the caller's cancellation token unchanged.
    fn checkpoint(&self) -> Result<(), Cancelled> {
        Ok(())
    }
}

/// Ordinary synchronous callers need no publication channel or export work.
pub struct NoopPublisher;

impl<N: WordElement> StagePublisher<N> for NoopPublisher {
    fn publish(&mut self, _model: &HybridAnalysis<N>) {}
}

#[cfg(test)]
mod tests;
