//! Issue #111 acceptance through real bytecode and the public analysis boundary.
//!
//! Unknown values remain unknown inputs: fixtures never obtain a concrete solver
//! model and feed it back as a snapshot observation. Precision is checked by the
//! program's later reads and branches, independently of state representation.

#[path = "symbolic_state/bytes.rs"]
mod bytes;
#[path = "symbolic_state/calls.rs"]
mod calls;
#[path = "symbolic_state/convergence.rs"]
mod convergence;
#[path = "symbolic_state/creation.rs"]
mod creation;
#[path = "symbolic_state/logs.rs"]
mod logs;
#[path = "symbolic_state/memory.rs"]
mod memory;
#[path = "symbolic_state/oracle.rs"]
mod oracle;
#[path = "symbolic_state/storage.rs"]
mod storage;
#[path = "symbolic_state/support.rs"]
mod support;
#[path = "symbolic_state/transfers.rs"]
mod transfers;
