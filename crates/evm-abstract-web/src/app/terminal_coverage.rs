//! Shared canvas/live text keeps semantic terminal coverage independent of
//! detailed closure. Absence of a certificate carries no inferred cause.

use evm_abstract_protocol::{AnalysisReport, AnalysisStatus};

pub(super) const COVERED: &str = "Root terminal results conservatively covered";
pub(super) const NOT_ATTACHED: &str = "Root terminal certificate not attached";

pub(super) fn status(report: &AnalysisReport) -> Option<&'static str> {
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
