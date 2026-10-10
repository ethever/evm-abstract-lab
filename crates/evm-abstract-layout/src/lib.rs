//! Viewport-independent geometry for a captured control-flow graph.
//!
//! Callers measure their actual card and label contents before layout. Spatial
//! groups preserve every leaf and edge identity; they are not analysis states,
//! SSA merges, or evidence that a sequence of aggregate edges is executable.
//! ELK objects stay private and local to one synchronous layout call. The input
//! and output are owned Rust data suitable for a future background executor.

mod engine;
mod geometry;
mod groups;
mod model;
#[cfg(test)]
mod tests;

pub use geometry::{Point, Rect, Size};
pub use model::{
    Edge, EdgeGeometry, EdgeKind, Group, GroupGeometry, GroupId, GroupKind, Input, Label, Layout,
    LayoutError, Node,
};

/// Compute groups and layered geometry with a fixed policy and no viewport.
pub fn layout(input: &Input) -> Result<Layout, LayoutError> {
    let groups = groups::identify(input)?;
    engine::layout(input, &groups)
}
