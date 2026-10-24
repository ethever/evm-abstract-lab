//! Shared canvas/live text keeps semantic terminal coverage independent of
//! detailed closure. Absence of a certificate carries no inferred cause.

use evm_abstract_protocol::{AnalysisReport, AnalysisStatus};

pub(super) const COVERED: &str = "Root terminal results conservatively covered";
pub(super) const NOT_ATTACHED: &str = "Root terminal certificate not attached";
pub(super) const NATIVE_AUXILIARY: &str = "Native terminal auxiliary available";

pub(super) fn status(report: &AnalysisReport) -> Option<&'static str> {
    if let Some(hybrid) = &report.hybrid {
        let native = report.graph(evm_abstract_protocol::AnalysisNamespace::Native);
        if native.is_some_and(|native| native.root_terminal_coverage.is_some()) {
            return Some(NATIVE_AUXILIARY);
        }
        return (hybrid.native_coverage().requested_native_terminal_coverage
            && native.is_none_or(|native| {
                native.status != AnalysisStatus::Converged || !native.ssa.complete
            }))
        .then_some(NOT_ATTACHED);
    }
    if report.root_terminal_coverage.is_some() {
        Some(COVERED)
    } else if report.metadata.limits.root_terminal_coverage
        && (report.status != AnalysisStatus::Converged || !report.ssa.complete)
    {
        Some(NOT_ATTACHED)
    } else {
        None
    }
}

pub(super) fn detail_status(report: &AnalysisReport) -> &'static str {
    if let Some(hybrid) = &report.hybrid {
        return if hybrid.coverage.baseline_current {
            "Global model converged"
        } else {
            "Global model incomplete"
        };
    }
    if report.status == AnalysisStatus::Converged && report.ssa.complete {
        "Model converged"
    } else if report.root_terminal_coverage.is_some() {
        "Detail incomplete / partial SSA"
    } else {
        "Incomplete / partial SSA"
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) mod tests;
