//! Read-only presentation of worker activity and the independently retained
//! report. It never changes transport generations or grants model currentness.

use evm_abstract_protocol::{
    AnalysisReport, AnalysisStatus, HybridDetailState, JobId, RichExportOutcome,
};

use super::{Task, TaskPhase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskIntent {
    Cancel,
}

/// Availability belongs to the visible report, independently of worker phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelPresentation {
    /// No validated report is visible.
    None,
    /// Report belongs to this job; readiness is its recorded model currentness.
    Current {
        /// The model records closed coverage under its input qualification.
        ready: bool,
    },
    /// A retained result belongs to an earlier job or local input.
    Previous {
        /// The retained model's own coverage remains closed.
        ready: bool,
    },
}

/// Optional detail has its own lifecycle; a closed model remains usable when
/// detail is incomplete, cancelled or refused by an export policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailPresentation {
    /// No detail qualification for the current job has arrived.
    Unavailable,
    /// Optional preparation or export continues while a model is available.
    Running,
    /// Detail has pending or uncertified work.
    Incomplete,
    /// Detail is available under the report's explicit qualification.
    Ready,
    /// The submitted policy selected no detail.
    Disabled,
    /// A typed export refusal retained the model.
    Refused,
    /// Cancellation retained the model.
    Cancelled,
}

/// A compact immutable status for shell chrome and assistive clients.
pub struct TaskPresentation {
    /// Current server task, as an exact integer suitable for copying.
    pub job_id: Option<JobId>,
    /// Task that owns the visible immutable result.
    pub report_job_id: Option<JobId>,
    /// Independently available model and its relationship to the current job.
    pub model: ModelPresentation,
    /// Optional detail activity for the current job.
    pub detail: DetailPresentation,
    /// Cooperative cancellation can be requested.
    pub cancellable: bool,
    /// Known worker or result-transfer activity continues.
    pub busy: bool,
    /// Raw task lifecycle label, independent of model readiness.
    pub phase: &'static str,
    /// Retained typed task failure rendered at the presentation boundary.
    pub error: Option<String>,
    activity: &'static str,
}

impl TaskPresentation {
    /// Worker activity, qualified by independently available model data.
    pub fn label(&self) -> &'static str {
        self.activity
    }
}

impl Task {
    pub(super) fn presentation(
        &self,
        report: Option<&AnalysisReport>,
        report_job_id: Option<JobId>,
        previous_report: bool,
    ) -> TaskPresentation {
        let standalone = report.is_some()
            && report_job_id.is_none()
            && !previous_report
            && self.phase == TaskPhase::Ready;
        let job_id = self
            .snapshot
            .as_ref()
            .filter(|_| !standalone)
            .map(|snapshot| snapshot.id);
        let previous = previous_report
            || (report.is_some() && report_job_id.is_some() && report_job_id != job_id);
        let model = report.map_or(ModelPresentation::None, |report| {
            let ready = report.hybrid.as_ref().map_or_else(
                || {
                    report
                        .minimum
                        .as_ref()
                        .map_or(report.status == AnalysisStatus::Converged, |model| {
                            model.current
                        })
                },
                |hybrid| hybrid.coverage.baseline_current,
            );
            if previous {
                ModelPresentation::Previous { ready }
            } else {
                ModelPresentation::Current { ready }
            }
        });
        let active_model = matches!(model, ModelPresentation::Current { ready: true });
        let detail =
            report
                .filter(|_| !previous)
                .map_or(DetailPresentation::Unavailable, |report| {
                    if matches!(report.rich_export, RichExportOutcome::Refused(_)) {
                        return DetailPresentation::Refused;
                    }
                    if matches!(report.rich_export, RichExportOutcome::Cancelled) {
                        return DetailPresentation::Cancelled;
                    }
                    if self.phase == TaskPhase::Cancelled && report.native.is_none() {
                        return DetailPresentation::Cancelled;
                    }
                    let Some(hybrid) = &report.hybrid else {
                        return report.native.as_ref().map_or(
                            DetailPresentation::Unavailable,
                            |graph| {
                                if graph.ssa.complete {
                                    DetailPresentation::Ready
                                } else {
                                    DetailPresentation::Incomplete
                                }
                            },
                        );
                    };
                    // Retained RPC detail keeps its own input revision and binding.
                    match hybrid.native_coverage().detail {
                        HybridDetailState::Disabled | HybridDetailState::ProcessPolicy => {
                            DetailPresentation::Disabled
                        }
                        _ if self.busy()
                            && active_model
                            && report.native.is_none()
                            && matches!(report.rich_export, RichExportOutcome::NotRequested) =>
                        {
                            DetailPresentation::Running
                        }
                        HybridDetailState::Current if report.native.is_some() => {
                            DetailPresentation::Ready
                        }
                        HybridDetailState::Current => DetailPresentation::Unavailable,
                        _ if self.phase == TaskPhase::Cancelled => DetailPresentation::Cancelled,
                        _ if self.busy() && active_model => DetailPresentation::Running,
                        _ => DetailPresentation::Incomplete,
                    }
                });
        let activity = match (active_model, self.phase, detail) {
            (true, TaskPhase::Cancelling, _) => "Model ready · cancelling detail",
            (true, TaskPhase::Cancelled, _) => "Model ready · detail cancelled",
            (true, _, DetailPresentation::Refused) => "Model ready · rich export refused",
            (true, _, DetailPresentation::Running) => "Model ready · detail running",
            (true, TaskPhase::Failed, _) => "Model retained · task failed",
            (true, _, DetailPresentation::Incomplete) => "Model ready · detail incomplete",
            (true, _, DetailPresentation::Disabled) => "Model ready · detail disabled",
            (true, _, DetailPresentation::Ready) => "Model ready · detail ready",
            (true, _, DetailPresentation::Cancelled) => "Model ready · detail cancelled",
            (true, _, DetailPresentation::Unavailable) => "Model ready · detail unavailable",
            (false, _, _)
                if !self.busy() && matches!(model, ModelPresentation::Current { ready: false }) =>
            {
                "Model available · coverage incomplete"
            }
            _ => self.label(),
        };
        TaskPresentation {
            job_id,
            report_job_id,
            model,
            detail,
            cancellable: self.cancellable(),
            busy: self.busy(),
            phase: self.label(),
            error: self.error.as_ref().map(ToString::to_string).or_else(|| {
                report.filter(|_| !previous).and_then(|report| {
                    if let RichExportOutcome::Refused(error) = &report.rich_export {
                        Some(error.message.clone())
                    } else {
                        None
                    }
                })
            }),
            activity,
        }
    }
}

impl crate::app::Workspace {
    /// Worker activity and visible model readiness, with exact copyable job IDs.
    pub fn task_presentation(&self) -> TaskPresentation {
        self.task.presentation(
            self.report.as_ref(),
            self.report_job_id,
            self.previous_report,
        )
    }
}
