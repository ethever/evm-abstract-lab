//! Client waiting and first-view work use one monotonic clock. Server phase
//! durations come exclusively from the backend's immutable timing snapshots.

use egui::Ui;
use evm_abstract_protocol::{AnalysisReport, AnalysisTimings};

/// Seconds on a monotonic clock shared by submission, transport and view setup.
pub(crate) fn now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.performance())
            .expect("the browser supplies the monotonic performance clock")
            .now()
            / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
    }
}

pub(crate) fn elapsed_ns(started: f64, ended: f64) -> u64 {
    ((ended - started).max(0.0) * 1_000_000_000.0) as u64
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Wait {
    started: Option<f64>,
    ended: Option<f64>,
}

impl Wait {
    pub(super) fn start(&mut self, now: f64) {
        self.started = Some(now);
        self.ended = None;
    }

    pub(super) fn finish(&mut self, now: f64) {
        if self.started.is_some() && self.ended.is_none() {
            self.ended = Some(now);
        }
    }

    pub(super) fn elapsed(&self, now: f64) -> Option<u64> {
        self.started
            .map(|start| elapsed_ns(start, self.ended.unwrap_or(now)))
    }

    pub(super) fn running(&self) -> bool {
        self.started.is_some() && self.ended.is_none()
    }

    pub(super) fn summary(&self, now: f64) -> Option<String> {
        self.elapsed(now).map(|duration| {
            format!(
                "{}: {:.2} s",
                if self.running() {
                    "Elapsed"
                } else {
                    "Total wait"
                },
                duration as f64 / 1_000_000_000.0
            )
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Frontend {
    pub(super) transfer_ns: u64,
    pub(super) parsing_ns: u64,
    pub(super) view_ns: u64,
}

impl Frontend {
    pub(super) fn total_ns(&self) -> u64 {
        self.transfer_ns
            .saturating_add(self.parsing_ns)
            .saturating_add(self.view_ns)
    }

    pub(super) fn summary(&self) -> String {
        format!(
            "Frontend preparation: {}; Result transfer: {}; JSON parsing: {}; First view setup: {}",
            format_ns(self.total_ns()),
            format_ns(self.transfer_ns),
            format_ns(self.parsing_ns),
            format_ns(self.view_ns)
        )
    }
}

/// Client measurements belong to this report even while another task runs.
#[derive(Clone, Debug)]
pub(super) struct ReportTiming {
    pub(super) generation: u64,
    pub(super) wait: Wait,
    pub(super) frontend: Frontend,
    pub(super) pending: bool,
}

impl ReportTiming {
    pub(super) fn record_view(&mut self, duration_ns: u64, ready: bool, now: f64) {
        if !self.pending {
            return;
        }
        self.frontend.view_ns = self.frontend.view_ns.saturating_add(duration_ns);
        if ready {
            self.wait.finish(now);
            self.pending = false;
        }
    }
}

/// Snapshot of the task bar used by the inspector. Visible report timing stays
/// separate from the current task's progress and is explicitly labeled.
#[derive(Clone, Debug, Default)]
pub(super) struct Details {
    pub(super) task_wait: Option<String>,
    pub(super) task_label: &'static str,
    pub(super) task_backend: Option<AnalysisTimings>,
    pub(super) report: Option<ReportTiming>,
    pub(super) previous: bool,
}

pub(super) fn format_ns(ns: u64) -> String {
    // Two decimals remain easy to compare with the live elapsed clock. Small
    // backend phases retain their measured nonzero duration instead of 0.00 s.
    if ns > 0 && ns < 10_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.2} s", ns as f64 / 1_000_000_000.0)
    }
}

pub(super) fn backend_summary(timing: &AnalysisTimings) -> String {
    format!(
        "Backend processing: {}; Queue: {}; RPC: {}; Abstract analysis: {}; SSA construction: {}; Report projection: {}; Input validation: {}",
        format_ns(timing.processing_ns),
        format_ns(timing.queue_ns),
        format_ns(timing.pinning_ns.saturating_add(timing.acquiring_ns)),
        format_ns(timing.analyzing_ns),
        format_ns(timing.building_ssa_ns),
        format_ns(timing.projecting_ns),
        format_ns(timing.validating_ns),
    )
}

pub(super) fn show(ui: &mut Ui, report: &AnalysisReport, details: &Details) {
    if let Some(wait) = &details.task_wait {
        ui.label(format!("Current task · {} · {wait}", details.task_label));
    }
    if details.previous {
        if let Some(backend) = &details.task_backend {
            ui.label("Current task backend");
            backend_rows(ui, backend, "current_task_timing");
        }
        ui.separator();
        ui.label("Previous report timing");
    } else {
        ui.label("Report timing");
    }
    backend_rows(ui, &report.metadata.timings, "report_timing");
    if let Some(timing) = &details.report {
        ui.separator();
        if let Some(wait) = timing.wait.summary(now()) {
            ui.label(wait);
        }
        egui::Grid::new("frontend_timing")
            .num_columns(2)
            .show(ui, |ui| {
                for (name, duration) in [
                    ("Frontend preparation", timing.frontend.total_ns()),
                    ("Result transfer", timing.frontend.transfer_ns),
                    ("JSON parsing", timing.frontend.parsing_ns),
                    ("First view setup", timing.frontend.view_ns),
                ] {
                    ui.label(name);
                    ui.label(format_ns(duration));
                    ui.end_row();
                }
            });
        if timing.pending {
            ui.label("Preparing the first visible result view…");
        }
    }
    ui.separator();
    ui.label("Backend phases are server measurements. RPC combines snapshot pinning and cumulative acquisition; abstract analysis includes local relation propagation.");
    ui.label("Backend processing excludes queueing and result transfer. Total wait runs from submission through the first visible view, including polling and connection delays.");
    ui.label("First view setup includes report installation and actual initial layout/drawing preparation. Hidden views remain lazy; later navigation and GPU presentation are not included.");
}

fn backend_rows(ui: &mut Ui, timing: &AnalysisTimings, id: &str) {
    egui::Grid::new(id).num_columns(2).show(ui, |ui| {
        for (name, duration) in [
            ("Backend processing", timing.processing_ns),
            ("Queue", timing.queue_ns),
            ("Input validation", timing.validating_ns),
            (
                "RPC · combined",
                timing.pinning_ns.saturating_add(timing.acquiring_ns),
            ),
            ("Snapshot pinning", timing.pinning_ns),
            ("RPC acquisition", timing.acquiring_ns),
            ("Abstract analysis", timing.analyzing_ns),
            ("SSA construction", timing.building_ssa_ns),
            ("Report projection", timing.projecting_ns),
        ] {
            ui.label(name);
            ui.label(format_ns(duration));
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests;
