//! Isolated compilation of the creation rules against a frozen foundation API.
//! Integration into Body/Resume is deliberately owned by the foundation lane.

pub use evm_abstract::analysis::process_creation as creation;
pub mod analysis {
    pub use evm_abstract::analysis::{OutcomeKind, ProcessAction, ProcessOwner};
}
pub mod resource {
    pub use evm_abstract::resource::WorkBudget;
}

#[path = "support/native.rs"]
mod native;
#[path = "process_creation/rules.rs"]
mod rules;
#[path = "process_creation/witness.rs"]
mod witness;
