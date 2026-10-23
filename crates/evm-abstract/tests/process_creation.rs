//! Isolated compilation of the creation rules against a frozen foundation API.
//! Integration into Body/Resume is deliberately owned by the foundation lane.

#[path = "process_creation/contract.rs"]
pub mod process_contract;

/// Provisional foundation imports; replace this harness with production imports
/// when the foundation and creation changes are combined.
pub mod analysis {
    pub use crate::process_contract::{ProcessAction, ProcessOwner};
    pub use evm_abstract::analysis::OutcomeKind;
}

/// The real cumulative ledger is shared with the production creation helper.
pub mod resource {
    pub use evm_abstract::resource::WorkBudget;
}

#[path = "../src/analysis/transfer/procedure/process/creation.rs"]
pub mod creation;

#[path = "support/native.rs"]
mod native;
#[path = "process_creation/rules.rs"]
mod rules;
#[path = "process_creation/witness.rs"]
mod witness;
