//! Browser analysis workspace. Rendering and interaction are independent of the
//! browser transport, and consume only the shared protocol's typed snapshot.

mod app;
mod controls;
mod notation;
mod palette;
mod report;
mod ssa_display;
mod widgets;

mod framework;

pub use app::{Command, JobOperation, Message, TransportError, Workspace};

#[cfg(target_arch = "wasm32")]
pub use framework::start;

#[cfg(test)]
mod tests;
