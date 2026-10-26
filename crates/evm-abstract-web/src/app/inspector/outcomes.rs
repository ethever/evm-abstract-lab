//! Final root outcomes keep returned data and finalized store paired.

use super::{Inspector, Selection, buffers, scroll, stores, terminal, value};
use crate::{
    controls::{self, Density},
    notation, palette,
    report::ReportView,
};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;

impl Inspector {
    pub(super) fn outcomes(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
    ) {
        scroll(ui, "outcome_details", |ui| {
            terminal::show(ui, report);
            if super::super::terminal_coverage::status(report.model).is_some() {
                ui.label(RichText::new("Observed terminal outcomes").strong());
            }
            self.observed_outcomes(ui, report, selection);
        });
    }

    fn observed_outcomes(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
    ) {
        if report.outcomes.is_empty() {
            ui.label("No terminal root outcome recorded. Check coverage frontiers.");
            return;
        }
        self.outcome = self.outcome.min(report.outcomes.len() - 1);
        let selected = &report.outcomes[self.outcome];
        egui::ComboBox::from_id_salt("outcome_selection")
            .width(ui.available_width().min(340.0))
            .selected_text(notation::widget(
                ui,
                format!(
                    "Outcome {} / {} · {} · {:?}",
                    Symbol::Outcome(self.outcome),
                    report.outcomes.len(),
                    Symbol::State(selected.state),
                    selected.kind
                ),
            ))
            .show_ui(ui, |ui| {
                for (index, outcome) in report.outcomes.iter().enumerate() {
                    if controls::tab_value(
                        ui,
                        &mut self.outcome,
                        index,
                        format!(
                            "Outcome {} · {} · {:?}",
                            Symbol::Outcome(index),
                            Symbol::State(outcome.state),
                            outcome.kind
                        ),
                        Density::Compact,
                    )
                    .clicked()
                    {
                        *selection = Selection {
                            value: None,
                            state: Some(outcome.state),
                            pc: None,
                        };
                    }
                }
            });
        let outcome = &report.outcomes[self.outcome];
        if let Some(data) = report.byte_arrays.get(outcome.data) {
            ui.horizontal(|ui| {
                ui.label("Return/revert length");
                value::cell(ui, &data.length);
            });
            ui.collapsing("Return/revert bytes", |ui| {
                ui.allocate_ui(egui::vec2(ui.available_width(), 120.0), |ui| {
                    buffers::show(ui, report, outcome.data, "Outcome bytes")
                });
            });
        }
        if let Some(store) = report.stores.get(outcome.store) {
            ui.label(RichText::new("Finalized store versus initial observations").strong());
            ui.label(
                RichText::new(
                    "Abstract differences can include joins and refinements as well as writes.",
                )
                .small()
                .color(palette::MUTED),
            );
            stores::changes(ui, report, store);
        }
    }
}
