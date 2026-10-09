//! Concrete cancellation tokens shared by a caller and its analysis thread.
//!
//! A scoped token is restored on every exit path. Uncontrolled library calls
//! have no token and keep their original solver/resource-limit semantics.

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::ThreadId,
};

#[derive(Debug, Default)]
struct State {
    cancelled: AtomicBool,
    wake: Condvar,
    gate: Mutex<()>,
}

/// One-way cancellation signal; the default token is deliberately disabled.
#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    state: Option<Arc<State>>,
}

impl Cancellation {
    /// Create an enabled token for one independently cancellable operation.
    pub fn new() -> Self {
        Self {
            state: Some(Arc::new(State::default())),
        }
    }

    /// Whether an owner can request cancellation of this operation.
    pub fn is_enabled(&self) -> bool {
        self.state.is_some()
    }

    /// Signal cancellation and wake any native-interrupt monitor.
    pub fn cancel(&self) {
        if let Some(state) = &self.state {
            state.cancelled.store(true, Ordering::Release);
            self.wake();
        }
    }

    /// Fast cooperative checkpoint; cancellation is irreversible.
    pub fn is_cancelled(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.cancelled.load(Ordering::Acquire))
    }

    /// Wake a monitor after its protected operation has finished.
    pub(crate) fn wake(&self) {
        if let Some(state) = &self.state {
            let _guard = state
                .gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.wake.notify_all();
        }
    }

    /// Wait without polling until either cancellation or operation completion.
    pub(crate) fn wait(&self, finished: &AtomicBool) {
        let Some(state) = &self.state else { return };
        let mut guard = state
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !self.is_cancelled() && !finished.load(Ordering::Acquire) {
            guard = state
                .wake
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

// Rust's thread_local! macro creates function-pointer accessors, which this
// workspace forbids. This concrete registry is held only while looking up or
// replacing a worker's signal; native calls and work-budget checks never hold
// the registry lock. Entries disappear when their outermost scope returns.
static SCOPES: Mutex<Vec<(ThreadId, Cancellation)>> = Mutex::new(Vec::new());

struct Restore {
    thread: ThreadId,
    previous: Option<Cancellation>,
}
impl Drop for Restore {
    fn drop(&mut self) {
        let mut scopes = SCOPES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = scopes.iter().position(|(thread, _)| *thread == self.thread) {
            if let Some(previous) = self.previous.take() {
                scopes[index].1 = previous;
            } else {
                scopes.swap_remove(index);
            }
        }
    }
}

/// Run on the current thread with a token, restoring a containing scope on exit.
pub fn with_cancellation<T>(token: &Cancellation, operation: impl FnOnce() -> T) -> T {
    let thread = std::thread::current().id();
    let previous = {
        let mut scopes = SCOPES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, current)) = scopes.iter_mut().find(|(owner, _)| *owner == thread) {
            Some(std::mem::replace(current, token.clone()))
        } else {
            scopes.push((thread, token.clone()));
            None
        }
    };
    let _restore = Restore { thread, previous };
    operation()
}

/// Current analysis token. Cloning preserves the cancellation identity.
pub fn current_cancellation() -> Cancellation {
    let thread = std::thread::current().id();
    SCOPES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(owner, _)| *owner == thread)
        .map(|(_, cancellation)| cancellation.clone())
        .unwrap_or_default()
}

struct Finish<'a> {
    cancellation: &'a Cancellation,
    finished: &'a AtomicBool,
}
impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        self.cancellation.wake();
    }
}

/// The monitor only delivers a native interrupt; all solver work stays on the
/// calling analysis thread. Its scope ends before the native handle is released.
pub(crate) fn interruptible<T>(
    cancellation: &Cancellation,
    interrupt: impl FnOnce() + Send,
    operation: impl FnOnce() -> T,
) -> T {
    if !cancellation.is_enabled() {
        return operation();
    }
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let _finish = Finish {
            cancellation,
            finished: &finished,
        };
        scope.spawn(|| {
            cancellation.wait(&finished);
            if cancellation.is_cancelled() && !finished.load(Ordering::Acquire) {
                interrupt();
            }
        });
        operation()
    })
}

#[cfg(test)]
mod tests;
