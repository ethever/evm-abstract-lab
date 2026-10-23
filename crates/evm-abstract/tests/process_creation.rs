//! Production CREATE/CREATE2 rule contracts and concrete EVM witnesses.
//! Installed Body/Resume coverage additionally lives in global_process.

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
