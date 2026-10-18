//! Scoped counters for synchronous tests without thread-local function pointers.

use std::{
    sync::{Mutex, MutexGuard},
    thread::{self, ThreadId},
};

pub(crate) struct Counter<T: Copy> {
    initial: T,
    entries: Mutex<Vec<(ThreadId, T)>>,
}

pub(crate) struct Scope<'a, T: Copy> {
    counter: &'a Counter<T>,
    owner: ThreadId,
}

impl<T: Copy> Counter<T> {
    pub(crate) const fn new(initial: T) -> Self {
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

    pub(crate) fn scope(&self) -> Scope<'_, T> {
        let owner = thread::current().id();
        let mut entries = self.entries();
        assert!(
            entries.iter().all(|(id, _)| *id != owner),
            "counter scope already active on this thread"
        );
        entries.push((owner, self.initial));
        Scope {
            counter: self,
            owner,
        }
    }

    // Hooks only mutate a counter under this lock. Observed work runs outside
    // it; storage is bounded by active test threads, not by hook invocations.
    pub(crate) fn update(&self, update: impl FnOnce(&mut T)) {
        let mut entries = self.entries();
        if entries.is_empty() {
            return;
        }
        let owner = thread::current().id();
        if let Some((_, value)) = entries.iter_mut().find(|(id, _)| *id == owner) {
            update(value);
        }
    }

    pub(crate) fn get(&self) -> T {
        let mut value = None;
        self.update(|current| value = Some(*current));
        value.expect("counter scope must be active on this thread")
    }

    pub(crate) fn replace(&self, value: T) -> T {
        let mut previous = None;
        self.update(|current| previous = Some(std::mem::replace(current, value)));
        previous.expect("counter scope must be active on this thread")
    }

    pub(crate) fn set(&self, value: T) {
        self.replace(value);
    }
}

impl<T: Copy> Drop for Scope<'_, T> {
    fn drop(&mut self) {
        let mut entries = self.counter.entries();
        if let Some(index) = entries.iter().position(|(id, _)| *id == self.owner) {
            entries.swap_remove(index);
        }
    }
}
