//! Context labels describe source, owner and conditional evidence separately.
//! They never use a process namespace as a captured physical account.

use super::{Selection, process, value};
use crate::{palette, report::ReportView};
use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisNamespace, FrameSnapshot, HybridNativeBinding};

pub(super) fn show(
    ui: &mut Ui,
    report: &ReportView<'_>,
    frame: Option<&FrameSnapshot>,
    selection: Selection,
) {
    if let Some(frame) = frame {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Context").small().color(palette::MUTED));
            ui.label(if frame.index == 0 {
                "Root frame".into()
            } else {
                format!("Call depth {}", frame.index + 1)
            });
            ui.label(if frame.is_static {
                "Read only · static"
            } else {
                "Writes permitted"
            });
            if !frame.context.is_empty() {
                ui.label(format!("{} retained jumps", frame.context.len()))
                    .on_hover_text(format!("Jump history: {:?}", frame.context));
            }
        });
        if let Some(key) = &frame.process {
            ui.label(format!(
                "{} · {}",
                process::source_label(&key.source),
                process::owner(&key.owner)
            ));
        } else {
            let input = frame.index == 0
                && frame.code_address.as_deref() == report.input_namespace()
                && report.input_namespace().is_some();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Code").small().color(palette::MUTED));
                ui.monospace(if input {
                    "Supplied input program"
                } else {
                    frame
                        .code_address
                        .as_deref()
                        .unwrap_or("Unrecorded code source")
                });
                ui.label(RichText::new("Owner").small().color(palette::MUTED));
                let owner = frame.storage_address.as_deref();
                ui.monospace(
                    if frame.index == 0 && owner == report.input_namespace() && owner.is_some() {
                        "Root input namespace"
                    } else {
                        owner.unwrap_or("Unrecorded owner")
                    },
                );
            });
        }
    }
    // A prefix retains the actual root condition even while inspecting a child.
    // A Baseline state with the same local ID must never borrow that condition.
    if report.identity().namespace() == AnalysisNamespace::Native
        && let Some(hybrid) = &report.hybrid
    {
        let binding = selection
            .state
            .and_then(|id| {
                hybrid.local_prefixes.iter().find(|prefix| {
                    prefix.namespace == AnalysisNamespace::Native && prefix.state == id
                })
            })
            .map_or(&hybrid.native_coverage().detail_binding, |prefix| {
                &prefix.binding
            });
        if let HybridNativeBinding::SymbolicOwnerNamespace(binding) = binding {
            ui.colored_label(palette::WARNING, format!("Source condition · root ADDRESS = {}", binding.namespace))
                .on_hover_text("This condition also qualifies child frames. Global MAY coverage retains other allowed root addresses.");
        }
    }
}

pub(super) fn process_input(ui: &mut Ui, input: &evm_abstract_protocol::ProcessOperands) {
    egui::Grid::new("minimum_process_operands")
        .num_columns(2)
        .show(ui, |ui| {
            value::named(ui, "Joined ADDRESS", &input.address);
            value::named(ui, "Joined CALLER", &input.caller);
            value::named(ui, "Joined CALLVALUE", &input.value);
        });
}
