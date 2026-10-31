//! Reusable EVM numeric constraints, static algebras and bounded symbolic facts.
//!
//! Pure numeric operations and paid facades require no world, frame, Store,
//! execution graph, RPC source, or analysis engine. Callers own their ledger and
//! identity scopes; provenance alone does not establish value equality.

mod domain;
pub mod payment;
mod resource;

pub use alloy_primitives::U256;
pub use analysis_kernel as kernel;
pub use domain::*;
pub use evm_resource::{ResourceError, WorkBudget};

#[cfg(test)]
mod test_instrumentation {
    pub(crate) use evm_resource::test_support::Counter;
}
