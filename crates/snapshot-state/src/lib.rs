//! Ordered mutable state with independent in-memory rollback checkpoints.
//!
//! [`OrderedMap`] preserves the same ordering and serialization with either
//! backend: the default uses `std::collections::BTreeMap`, while the `imbl`
//! feature uses a persistent ordered map with shared nodes and copy-on-write
//! mutations. Backend selection is compile-time and does not change state
//! semantics. These structures do not persist data to disk.
//!
//! [`Checkpoint`] owns one independent state version. Checkpoints can be cloned,
//! joined by a domain-specific consumer, and restored in any order; they are
//! deliberately not tied to a global journal or a last-in-first-out stack.
//! [`SharedCheckpoint`] shares a logically immutable captured version when
//! cloned and isolates it again before restoring into a mutable owner.

mod checkpoint;
mod ordered_map;
mod shared_checkpoint;

pub use checkpoint::Checkpoint;
pub use ordered_map::{IntoIter, Iter, OrderedMap};
pub use shared_checkpoint::SharedCheckpoint;

/// The map backend selected for this build, suitable for comparison metadata.
#[cfg(not(feature = "imbl"))]
pub const BACKEND: &str = "std";

/// The map backend selected for this build, suitable for comparison metadata.
#[cfg(feature = "imbl")]
pub const BACKEND: &str = "imbl";

#[cfg(test)]
mod tests;
