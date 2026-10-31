//! Shared cumulative ledger with execution-specific value/summary adapters.

pub use evm_resource::WorkBudget;
pub(crate) use evm_resource::{AdmissionStop, CompletionPermit, ResourceError as CloneError};

pub(crate) mod clone_payment;
mod summary;
pub(crate) use summary::SummaryError;
