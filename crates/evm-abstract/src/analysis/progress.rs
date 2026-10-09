//! Optional, bounded observations of one analysis, independent of any UI.
//!
//! Progress never changes execution semantics. A slow or disconnected consumer
//! cannot block analysis; reports contain the authoritative final counters.

use std::sync::mpsc::SyncSender;

/// Discrete work phases; acquisition and execution may repeat during discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Validate the declared inputs before execution or network access.
    Validating,
    /// Resolve the chain identity and exact block hash.
    Pinning,
    /// Acquire initial account or storage facts at that fixed snapshot.
    Acquiring,
    /// Execute the abstract worklist on the analysis thread.
    Analyzing,
    /// Build and verify the graph's SSA representation.
    BuildingSsa,
    /// Project the immutable result into an application report.
    Projecting,
}

/// A typed observation. Counters are cumulative across RPC discovery rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Enter a phase; this is not a completion percentage.
    Phase(Phase),
    /// Latest execution counters at a worklist checkpoint.
    Execution {
        /// Allocated native states, including discarded discovery rounds.
        states: usize,
        /// Executed block transfers, including revisits.
        transfers: usize,
        /// Reserved cumulative work units.
        work: usize,
    },
    /// Latest acquisition counters at a completed request or round boundary.
    Acquisition {
        /// Discovery round; zero denotes initial acquisition.
        round: usize,
        /// Accounts installed in the fixed snapshot.
        accounts: usize,
        /// Initial storage slots observed in that snapshot.
        slots: usize,
        /// RPC requests attempted, including failures and identity checks.
        requests: usize,
    },
}

/// Cloneable concrete channel endpoint; default analyses have no observer.
#[derive(Clone, Debug, Default)]
pub struct Observer {
    sender: Option<SyncSender<Event>>,
}

impl Observer {
    /// Observe events through a caller-owned bounded channel.
    pub fn new(sender: SyncSender<Event>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// Publish a best-effort observation without blocking the analysis thread.
    pub fn emit(&self, event: Event) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(event);
        }
    }

    /// Announce a discrete phase.
    pub fn phase(&self, phase: Phase) {
        self.emit(Event::Phase(phase));
    }
}
