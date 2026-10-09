//! Explicit application control for one synchronous, single-threaded analysis.
//!
//! Cancellation requests do not mean resources have already been released.
//! Applications acknowledge completion only after the controlled call returns.

use super::progress::Observer;
pub use embedded_smt::Cancellation as CancelToken;

/// Cancellation was observed before starting the next analysis operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("analysis cancelled")]
pub struct Cancelled;

/// Concrete cancellation and observation handles, with no dynamic callbacks.
#[derive(Clone, Debug, Default)]
pub struct Control {
    cancellation: CancelToken,
    observer: Observer,
}

impl Control {
    /// Create a cancellable operation with a bounded progress observer.
    pub fn new(observer: Observer) -> Self {
        Self {
            cancellation: CancelToken::new(),
            observer,
        }
    }

    /// Attach progress without enabling cancellation for an ordinary caller.
    pub fn with_observer(observer: Observer) -> Self {
        Self {
            cancellation: CancelToken::default(),
            observer,
        }
    }

    /// Signal shared with the task owner and network/solver boundaries.
    pub fn cancellation(&self) -> &CancelToken {
        &self.cancellation
    }

    /// Best-effort typed progress observations.
    pub fn observer(&self) -> &Observer {
        &self.observer
    }

    /// Stop before starting another operation when cancellation was requested.
    pub fn checkpoint(&self) -> Result<(), Cancelled> {
        if self.cancellation.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Scope the native solver's cancellation context to this worker thread.
    pub fn scope<T>(&self, operation: impl FnOnce() -> T) -> T {
        embedded_smt::with_cancellation(&self.cancellation, operation)
    }
}
