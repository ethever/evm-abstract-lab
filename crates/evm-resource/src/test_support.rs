//! Scoped counters for synchronous tests without thread-local function pointers.

use std::{
    sync::{Mutex, MutexGuard},
    thread::{self, ThreadId},
};

/// Thread-scoped observations; hooks outside an active scope are ignored.
pub struct Counter<T> {
    initial: T,
    entries: Mutex<Vec<(ThreadId, T)>>,
}

/// RAII owner that removes this thread's observations on drop.
pub struct Scope<'a, T> {
    counter: &'a Counter<T>,
    owner: ThreadId,
}

impl<T> Counter<T> {
    /// Create a counter with an initial value for each thread scope.
    pub const fn new(initial: T) -> Self {
        Self {
            initial,
            entries: Mutex::new(Vec::new()),
        }
    }

    fn entries(&self) -> MutexGuard<'_, Vec<(ThreadId, T)>> {
        // A failed assertion or counter update must not strand another test's
        // active entry, or prevent its scope from cleaning up during unwinding.
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // Only scope initialization clones the template. Non-Copy observed values
    // can then move into and out of the scope through replace without cloning.
    /// Begin one scope on this thread; nested scopes for this counter are rejected.
    pub fn scope(&self) -> Scope<'_, T>
    where
        T: Clone,
    {
        let owner = thread::current().id();
        let mut entries = self.entries();
        assert!(
            entries.iter().all(|(id, _)| *id != owner),
            "counter scope already active on this thread"
        );
        entries.push((owner, self.initial.clone()));
        Scope {
            counter: self,
            owner,
        }
    }

    // Hooks only mutate a counter under this lock. Observed work runs outside
    // it; storage is bounded by active test threads, not by hook invocations.
    /// Update only this thread’s active observation, if any.
    pub fn update(&self, update: impl FnOnce(&mut T)) {
        let mut entries = self.entries();
        if entries.is_empty() {
            return;
        }
        let owner = thread::current().id();
        if let Some((_, value)) = entries.iter_mut().find(|(id, _)| *id == owner) {
            update(value);
        }
    }

    /// Read the current observation; requires an active scope.
    pub fn get(&self) -> T
    where
        T: Copy,
    {
        let mut value = None;
        self.update(|current| value = Some(*current));
        value.expect("counter scope must be active on this thread")
    }

    /// Move an observation out and install its replacement; requires an active scope.
    pub fn replace(&self, value: T) -> T {
        let mut previous = None;
        self.update(|current| previous = Some(std::mem::replace(current, value)));
        previous.expect("counter scope must be active on this thread")
    }

    /// Replace the current observation; requires an active scope.
    pub fn set(&self, value: T) {
        self.replace(value);
    }
}

impl<T> Drop for Scope<'_, T> {
    fn drop(&mut self) {
        let mut entries = self.counter.entries();
        if let Some(index) = entries.iter().position(|(id, _)| *id == self.owner) {
            entries.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests;
