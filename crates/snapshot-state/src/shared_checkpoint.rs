use serde::{Serialize, Serializer};
use std::{fmt, sync::Arc};

#[cfg(test)]
mod tests;

/// A shared immutable version of a complete mutable state.
///
/// Capturing clones the current state once; cloning this checkpoint only shares
/// that captured version. As with [`crate::Checkpoint`], the state's `Clone`
/// must isolate its writable contents. The captured state must also remain
/// logically immutable through `&T`, including any interior mutable handles.
/// Restoration moves a uniquely held version or clones a shared version before
/// handing it back to a mutable owner. Dropping a checkpoint never changes that
/// owner's current state. Serialization remains transparent to the state.
#[derive(PartialEq, Eq)]
pub struct SharedCheckpoint<T: Clone>(Arc<T>);

impl<T: Clone> Clone for SharedCheckpoint<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Clone> SharedCheckpoint<T> {
    /// Wrap an already-owned state version without another clone.
    pub fn from_state(state: T) -> Self {
        Self(Arc::new(state))
    }

    /// Capture a complete independent version of the current state.
    pub fn capture(state: &T) -> Self {
        Self::from_state(state.clone())
    }

    /// Inspect this version without exposing mutable access to it.
    pub fn state(&self) -> &T {
        self.0.as_ref()
    }

    /// Replace the current state without exposing a sibling's immutable version.
    pub fn restore(self, state: &mut T) {
        *state = match Arc::try_unwrap(self.0) {
            Ok(owned) => owned,
            Err(shared) => shared.as_ref().clone(),
        };
    }
}

impl<T: Clone + fmt::Debug> fmt::Debug for SharedCheckpoint<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Retain the original Checkpoint tuple layout using static dispatch.
        #[derive(Debug)]
        #[expect(
            dead_code,
            reason = "field is read by the derived Debug implementation"
        )]
        struct Checkpoint<'a, T>(&'a T);
        fmt::Debug::fmt(&Checkpoint(self.state()), f)
    }
}

impl<T: Clone + Serialize> Serialize for SharedCheckpoint<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.state().serialize(serializer)
    }
}
