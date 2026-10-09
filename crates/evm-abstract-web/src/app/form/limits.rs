//! Typed execution, precision and acquisition controls use protocol defaults.

use super::AnalysisForm;
use egui::Ui;
use evm_abstract_protocol::{DomainProfile, SmtProvider};

impl AnalysisForm {
    pub(super) fn limits(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Execution budget").show(ui, |ui| {
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
                        number(ui, label, value);
                    }
                });
            ui.checkbox(
                &mut self.limits.use_summaries,
                "Reuse complete callee summaries",
            );
        });
        egui::CollapsingHeader::new("Precision and solver").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Numeric domain");
                egui::ComboBox::from_id_salt("numeric_domain")
                    .selected_text(format!("{:?}", self.limits.domain_profile))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.limits.domain_profile,
                            DomainProfile::Product,
                            "Product",
                        );
                        ui.selectable_value(
                            &mut self.limits.domain_profile,
                            DomainProfile::ConstantsOnly,
                            "Constants only",
                        );
                    });
            });
            egui::Grid::new("precision_limits")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Jump context depth", &mut self.limits.context_depth),
                        ("Constants per value", &mut self.limits.max_constants),
                        ("Reduction rounds", &mut self.limits.reduction_rounds),
                        ("Scalar facts", &mut self.limits.max_facts),
                        ("Constraints", &mut self.limits.max_constraints),
                        ("Expression nodes", &mut self.limits.max_expression_nodes),
                        ("Expression depth", &mut self.limits.max_expression_depth),
                    ] {
                        number(ui, label, value);
                    }
                });
            ui.checkbox(
                &mut self.limits.relations_enabled,
                "Relational SMT reasoning",
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Solver");
                egui::ComboBox::from_id_salt("smt_provider")
                    .selected_text(format!("{:?}", self.limits.smt_provider))
                    .show_ui(ui, |ui| {
                        for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5]
                        {
                            ui.selectable_value(
                                &mut self.limits.smt_provider,
                                provider,
                                format!("{provider:?}"),
                            );
                        }
                    });
                ui.label("Resource limit")
                    .on_hover_text("Provider work allowance; not milliseconds");
                ui.add(egui::DragValue::new(&mut self.limits.smt_rlimit));
            });
        });
        egui::CollapsingHeader::new("RPC acquisition budget").show(ui, |ui| {
            egui::Grid::new("rpc_limits").num_columns(2).show(ui, |ui| {
                for (label, value) in [
                    ("Accounts", &mut self.limits.rpc_max_accounts),
                    ("Requests", &mut self.limits.rpc_max_requests),
                    ("Response bytes", &mut self.limits.rpc_max_response_bytes),
                ] {
                    number(ui, label, value);
                }
                ui.label("Timeout (ms)");
                ui.add(egui::DragValue::new(&mut self.limits.rpc_timeout_ms));
                ui.end_row();
            });
        });
    }
}

fn number(ui: &mut Ui, label: &str, value: &mut usize) {
    ui.label(label);
    ui.add(egui::DragValue::new(value));
    ui.end_row();
}
