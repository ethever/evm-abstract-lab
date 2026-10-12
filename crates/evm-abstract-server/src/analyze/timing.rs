//! Project native monotonic timing without relying on bounded progress events.

use evm_abstract::analysis::{
    control::Control,
    progress::{Observer, TimingSnapshot},
};
use evm_abstract_protocol::AnalysisTimings;

/// The outermost caller freezes processing on return, failure or panic unwinding.
/// Nested adapters borrow the worker session without resetting or ending it.
pub(crate) struct Guard {
    observer: Observer,
    owns_session: bool,
}

impl Guard {
    pub(crate) fn start(control: &Control) -> Self {
        let observer = control.observer().clone();
        let owns_session = observer.start_timing();
        Self {
            observer,
            owns_session,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.owns_session {
            self.observer.finish_timing();
        }
    }
}

pub(crate) fn project(snapshot: TimingSnapshot, queue_ns: u64) -> AnalysisTimings {
    AnalysisTimings {
        queue_ns,
        processing_ns: snapshot.elapsed_ns,
        validating_ns: snapshot.phases.validating_ns,
        pinning_ns: snapshot.phases.pinning_ns,
        acquiring_ns: snapshot.phases.acquiring_ns,
        analyzing_ns: snapshot.phases.analyzing_ns,
        building_ssa_ns: snapshot.phases.building_ssa_ns,
        projecting_ns: snapshot.phases.projecting_ns,
    }
}
