//! Analysis and local decompiler workspaces in one browser application.
//! Rendering consumes typed protocol data independently of browser transport.

mod app;
#[cfg(any(target_arch = "wasm32", test))]
mod application;
mod controls;
#[cfg(any(target_arch = "wasm32", test))]
mod decompile;
mod graph_view;
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
