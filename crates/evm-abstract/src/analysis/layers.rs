//! Code compilation, immutable initial observations, and execution are separate layers.
//!
//! A [`CodeCatalog`] contains no chain state. [`StateFacts`] only restricts the
//! same initial snapshot: an omitted fact stays unobserved, including zero and
//! account absence. [`AnalysisSession`] projects those facts into a fresh EVM
//! execution; refinement never edits a transaction Store or a rollback root.
//!
//! Compiled block semantics are shared across snapshots. After refinement the
//! engine creates fresh runtime state and can reuse certified pure stack deltas.
//! Stateful blocks and interactions execute again; no saved Store is transplanted.

mod catalog;
mod dependencies;
mod facts;
pub(in crate::analysis) mod payment;
mod session;

pub use catalog::{
    BlockFacts, CatalogError, CodeCatalog, CodeId, CodeMode, CompiledCode, FactKind,
};
pub use dependencies::{FactDependencies, FactDependency};
pub use facts::{
    CodeObservation, FactError, FactKey, FactRefinement, FactRevision, Observation, Observed,
    SnapshotId, StateFacts,
};
pub use session::{
    AnalysisSession, FactDemand, RefinementEffect, ReuseStats, SessionAnalysis, SessionError,
};

#[cfg(test)]
mod tests;
