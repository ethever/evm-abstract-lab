//! Public-API acceptance and independent concrete evidence for issue #124.
//! Every execution fixture is runtime bytecode; helpers only assemble labels
//! and inspect observable outputs, never emulate an unknown summary.

#[path = "support/native.rs"]
mod native;

#[path = "unknown_world/concrete.rs"]
mod concrete;
#[path = "unknown_world/contexts.rs"]
mod contexts;
#[path = "unknown_world/fixtures.rs"]
mod fixtures;
#[path = "unknown_world/interactions.rs"]
mod interactions;
#[path = "unknown_world/lifecycle.rs"]
mod lifecycle;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/reporting.rs"]
mod reporting;
#[path = "unknown_world/rollback.rs"]
mod rollback;
#[path = "unknown_world/static_callbacks.rs"]
mod static_callbacks;
#[path = "unknown_world/support.rs"]
mod support;
#[path = "unknown_world/targets.rs"]
mod targets;
#[path = "unknown_world/transport.rs"]
mod transport;
