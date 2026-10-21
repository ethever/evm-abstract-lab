//! Downstream semantic and protocol acceptance; no access to kernel internals.
use analysis_kernel::{Analyzer, LocalClosure, Meter};

#[path = "cooperation/atomicity.rs"]
mod atomicity;
#[path = "cooperation/bounds.rs"]
mod bounds;
#[path = "cooperation/candidates.rs"]
mod candidates;
#[path = "cooperation/capabilities.rs"]
mod capabilities;
#[path = "cooperation/contracts.rs"]
mod contracts;
#[path = "cooperation/cycle.rs"]
mod cycle;
#[path = "cooperation/generations.rs"]
mod generations;
#[path = "cooperation/markers.rs"]
mod markers;
#[path = "cooperation/protocol.rs"]
mod protocol;
#[path = "cooperation/staging.rs"]
mod staging;
#[path = "cooperation/support.rs"]
mod support;

#[path = "support/meter.rs"]
mod meter;

#[path = "support/analyzer.rs"]
mod analyzer;
