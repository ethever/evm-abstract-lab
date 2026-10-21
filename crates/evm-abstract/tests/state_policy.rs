//! External clients select complete-state semantics through the public API.
//! Concrete revm witnesses and retained value guarantees are the assertions;
//! state/update counts are only supplementary diagnostics.

#[path = "support/native.rs"]
mod native;

#[path = "state_policy/interruption.rs"]
mod interruption;
#[path = "state_policy/join.rs"]
mod join;
#[path = "state_policy/oracle.rs"]
mod oracle;
#[path = "state_policy/partition.rs"]
mod partition;
#[path = "state_policy/procedures.rs"]
mod procedures;
#[path = "state_policy/support.rs"]
mod support;
