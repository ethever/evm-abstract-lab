use serde::Serialize;

/// An opaque owned version of a complete mutable state.
///
/// Capturing delegates to the state's `Clone` implementation. State types must
/// clone their writable contents independently for rollback isolation: cloning a
/// shared mutable handle does not snapshot its pointee. Restoration moves the
/// captured state back into its owner. Successful operations need no commit
/// mutation: dropping their checkpoint leaves the current state intact, and an
/// ancestor checkpoint still covers all deeper changes. Serialization is
/// transparent to the state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Checkpoint<T>(T);

impl<T> Checkpoint<T> {
    /// Wrap an already-owned state version without another clone.
    pub fn from_state(state: T) -> Self {
        Self(state)
    }

    /// Inspect this version without exposing mutable access to it.
    pub fn state(&self) -> &T {
        &self.0
    }

    /// Replace the current state with this version, consuming the checkpoint.
    pub fn restore(self, state: &mut T) {
        *state = self.0;
    }
}

impl<T: Clone> Checkpoint<T> {
    /// Capture a complete independent version of the current state.
    pub fn capture(state: &T) -> Self {
        Self(state.clone())
    }
}
