//! Known bytecode recursion is checked against concrete revm execution, with
//! independent assertions for closure, call context and transaction effects.

#[path = "known_recursion/admission.rs"]
mod admission;
#[path = "known_recursion/captured.rs"]
mod captured;
#[path = "known_recursion/contexts.rs"]
mod contexts;
#[path = "known_recursion/effects.rs"]
mod effects;
#[path = "known_recursion/fixtures.rs"]
mod fixtures;
#[path = "known_recursion/oracle.rs"]
mod oracle;
#[path = "known_recursion/recursion.rs"]
mod recursion;
#[path = "known_recursion/resources.rs"]
mod resources;
#[path = "known_recursion/support.rs"]
mod support;
