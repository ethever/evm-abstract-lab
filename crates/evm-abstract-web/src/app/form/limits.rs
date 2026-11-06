//! Typed execution, precision and acquisition controls use protocol defaults.

use crate::controls::{self, Density};

use super::AnalysisForm;
use egui::{RichText, Ui};
use evm_abstract_protocol::NumericModel;

use crate::{framework, palette};

impl AnalysisForm {
    pub(super) fn limits(&mut self, ui: &mut Ui) {
        framework::section(ui, "Execution budget", |ui| {
            egui::Grid::new("execution_limits")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("States", &mut self.limits.max_states),
                        ("Transfers", &mut self.limits.max_transfers),
                        ("Logical work", &mut self.limits.max_work),
                        ("Call depth", &mut self.limits.max_call_depth),
                        ("Memory bytes", &mut self.limits.max_memory_bytes),
                    ] {
                        self.limit_inputs.number(ui, label, value);
                    }
                });
            framework::checkbox(
                ui,
                &mut self.limits.use_summaries,
                "Reuse complete callee summaries",
            );
            framework::checkbox(
                ui,
                &mut self.limits.root_terminal_coverage,
                "Prepare conservative root terminal coverage",
            );
            ui.label(
                RichText::new("Global control runs first; remaining work extends detailed paths. Terminal preparation belongs to the native detail allowance and can remain unavailable.")
                    .small()
                    .color(palette::MUTED),
            );
        });
        framework::section(ui, "Precision", |ui| {
            ui.horizontal(|ui| {
                ui.label("Numeric model");
                let observed_numeric_model = egui::ComboBox::from_id_salt("numeric_model")
                    .selected_text(match self.limits.numeric_model {
                        NumericModel::Product => "Product",
                        NumericModel::FiniteSet => "Finite set",
                    })
                    .show_ui(ui, |ui| {
                        controls::tab_value(
                            ui,
                            &mut self.limits.numeric_model,
                            NumericModel::Product,
                            "Product",
                            Density::Regular,
                        );
                        controls::tab_value(
                            ui,
                            &mut self.limits.numeric_model,
                            NumericModel::FiniteSet,
                            "Finite set",
                            Density::Regular,
                        );
                    });
                controls::observe(
                    ui,
                    &observed_numeric_model.response,
                    "numeric_model",
                    "combobox",
                    None,
                );
            });
            egui::Grid::new("precision_limits")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Detailed code sources", &mut self.limits.max_detail_regions),
                        ("Jump context depth", &mut self.limits.context_depth),
                        ("Constants per value", &mut self.limits.max_constants),
                        ("Reduction rounds", &mut self.limits.reduction_rounds),
                        ("Scalar facts", &mut self.limits.max_facts),
                        ("Constraints", &mut self.limits.max_constraints),
                        ("Expression nodes", &mut self.limits.max_expression_nodes),
                        ("Expression depth", &mut self.limits.max_expression_depth),
                    ] {
                        self.limit_inputs.number(ui, label, value);
                    }
                });
            ui.label(
                RichText::new("0 keeps only the Global model. Selected code sources share the remaining execution work.")
                    .small()
                    .color(palette::MUTED),
            );
            framework::checkbox(
                ui,
                &mut self.limits.relations_enabled,
                "Local path reasoning",
            );
        });
        framework::section(ui, "RPC acquisition budget", |ui| {
            egui::Grid::new("rpc_limits").num_columns(2).show(ui, |ui| {
                for (label, value) in [
                    ("Accounts", &mut self.limits.rpc_max_accounts),
                    ("Requests", &mut self.limits.rpc_max_requests),
                    ("Response bytes", &mut self.limits.rpc_max_response_bytes),
                ] {
                    self.limit_inputs.number(ui, label, value);
                }
                self.limit_inputs
                    .number(ui, "Timeout (ms)", &mut self.limits.rpc_timeout_ms);
            });
        });
    }
}
