//! Optional, bounded observations of one analysis, independent of any UI.
//!
//! Progress never changes execution semantics. A slow or disconnected consumer
//! cannot block analysis; reports contain the authoritative final counters.

use std::{
    sync::{Arc, Mutex, MutexGuard, mpsc::SyncSender},
    time::Instant,
};

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

/// Exclusive monotonic wall-clock time accumulated across all discovery rounds.
/// These measurements are independent of logical execution work and relation limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTimings {
    /// Input/configuration validation and preparation before the first phase switch.
    pub validating_ns: u64,
    /// Resolving the fixed chain identity and canonical block.
    pub pinning_ns: u64,
    /// Initial and incremental fixed-snapshot fact acquisition.
    pub acquiring_ns: u64,
    /// Abstract execution and preparation, including discarded discovery rounds.
    pub analyzing_ns: u64,
    /// SSA construction, normalization and verification.
    pub building_ssa_ns: u64,
    /// Immutable application-report projection.
    pub projecting_ns: u64,
}

impl PhaseTimings {
    fn add(&mut self, phase: Phase, elapsed_ns: u64) {
        let accumulated = match phase {
            Phase::Validating => &mut self.validating_ns,
            Phase::Pinning => &mut self.pinning_ns,
            Phase::Acquiring => &mut self.acquiring_ns,
            Phase::Analyzing => &mut self.analyzing_ns,
            Phase::BuildingSsa => &mut self.building_ssa_ns,
            Phase::Projecting => &mut self.projecting_ns,
        };
        *accumulated = accumulated.saturating_add(elapsed_ns);
    }
}

/// A live or frozen measurement of one caller-owned analysis session.
/// `started_at` is process-local; application reports export durations instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimingSnapshot {
    /// Actual start of the session, retained after freezing.
    pub started_at: Option<Instant>,
    /// Total wall-clock elapsed time since the session began.
    pub elapsed_ns: u64,
    /// Exclusive phase durations; the running phase is included in live snapshots.
    pub phases: PhaseTimings,
}

#[derive(Debug, Default)]
struct Timing {
    recorded: TimingSnapshot,
    active: Option<(Phase, Instant)>,
}

impl Timing {
    fn start(&mut self, now: Instant) -> bool {
        if self.active.is_some() {
            return false;
        }
        self.recorded = TimingSnapshot {
            started_at: Some(now),
            ..TimingSnapshot::default()
        };
        self.active = Some((Phase::Validating, now));
        true
    }

    fn phase(&mut self, next: Phase, now: Instant) {
        if let Some((previous, since)) = self.active {
            self.recorded.phases.add(previous, elapsed(since, now));
            self.active = Some((next, now));
        }
    }

    fn finish(&mut self, now: Instant) {
        if let Some((phase, since)) = self.active.take() {
            self.recorded.phases.add(phase, elapsed(since, now));
            self.recorded.elapsed_ns = elapsed(
                self.recorded
                    .started_at
                    .expect("running timing has a start"),
                now,
            );
        }
    }

    fn snapshot(&self, now: Instant) -> TimingSnapshot {
        let mut snapshot = self.recorded;
        if let Some((phase, since)) = self.active {
            snapshot.phases.add(phase, elapsed(since, now));
            snapshot.elapsed_ns = elapsed(
                snapshot.started_at.expect("running timing has a start"),
                now,
            );
        }
        snapshot
    }
}

fn elapsed(since: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(since).as_nanos()).unwrap_or(u64::MAX)
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
    timings: Arc<Mutex<Timing>>,
}

impl Observer {
    /// Observe events through a caller-owned bounded channel.
    pub fn new(sender: SyncSender<Event>) -> Self {
        Self {
            sender: Some(sender),
            ..Self::default()
        }
    }

    fn timing_state(&self) -> MutexGuard<'_, Timing> {
        self.timings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Begin optional timing in Validating, preserving an already-running session.
    /// A frozen session is replaced when the same observer is reused for new work.
    /// Returns true only to the caller that starts and owns the new session;
    /// nested callers must leave freezing to that owner.
    pub fn start_timing(&self) -> bool {
        self.timing_state().start(Instant::now())
    }

    /// Freeze the session after its caller finishes, fails, or acknowledges cancellation.
    /// Repeated calls cannot extend a completed measurement.
    pub fn finish_timing(&self) {
        self.timing_state().finish(Instant::now());
    }

    /// Read authoritative timing shared by all observer clones, including active work.
    /// Untimed analyses return an empty snapshot.
    pub fn timing(&self) -> TimingSnapshot {
        self.timing_state().snapshot(Instant::now())
    }

    /// Publish a best-effort observation without blocking the analysis thread.
    pub fn emit(&self, event: Event) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(event);
        }
    }

    /// Announce a discrete phase.
    pub fn phase(&self, phase: Phase) {
        {
            let mut timing = self.timing_state();
            if timing.active.is_some() {
                timing.phase(phase, Instant::now());
            }
        }
        self.emit(Event::Phase(phase));
    }
}

#[cfg(test)]
mod tests;
