//! Inspect the native joint terminal tuples without inventing CFG coordinates.
//! This module receives only an immutable, already validated report; it cannot
//! select a state or turn widened display values into new input authority.

use super::{buffers, context, stores, symbolic, value};
use crate::{notation, palette};
use egui::{RichText, Ui};
use evm_abstract_notation::{Symbol, WideSymbol};
use evm_abstract_protocol::{
    AnalysisReport, RootTerminalConfig, RootTerminalCoverage, RootTerminalDomainSpec,
    RootTerminalInputBinding, RootTerminalOutcome, RootTerminalSnapshot,
};

pub(super) fn show(ui: &mut Ui, report: &AnalysisReport) {
    let Some(coverage) = &report.root_terminal_coverage else {
        if let Some(status) = super::super::terminal_coverage::status(report) {
            ui.label(status);
            ui.separator();
        }
        return;
    };
    ui.label(RichText::new("Conservative root terminal coverage").strong());
    ui.label(super::super::terminal_coverage::COVERED);
    ui.label(
        "The alternatives conservatively cover terminal results for the original input. Each pairs its data with its finalized store; it does not establish that the alternative occurs.",
    );
    ui.collapsing("Original input binding", |ui| binding(ui, report, coverage));
    for (index, outcome) in coverage.outcomes.iter().enumerate() {
        // Keep all three alternatives separate from the observed-outcome
        // chooser, even when no detailed terminal state has been reached.
        ui.push_id(("root_terminal_alternative", index), |ui| {
            egui::CollapsingHeader::new(format!("{:?} alternative", outcome.kind))
                .default_open(true)
                .show(ui, |ui| alternative(ui, report, outcome));
        });
    }
    ui.separator();
}

fn alternative(ui: &mut Ui, report: &AnalysisReport, outcome: &RootTerminalOutcome) {
    if let Some(bytes) = report.byte_arrays.get(outcome.data) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Data snapshot #{} · length", outcome.data));
            value::cell(ui, &bytes.length);
        });
        ui.collapsing("Terminal bytes", |ui| {
            ui.allocate_ui(egui::vec2(ui.available_width(), 140.0), |ui| {
                buffers::show(ui, report, outcome.data, "Terminal alternative bytes");
            });
        });
    }
    let store_label = notation::widget(
        ui,
        format!("Finalized store {}", Symbol::Store(outcome.store)),
    );
    ui.collapsing(store_label, |ui| {
        if let Some(store) = report.stores.get(outcome.store) {
            stores::changes(ui, report, store);
        }
    });
}

fn binding(ui: &mut Ui, report: &AnalysisReport, coverage: &RootTerminalCoverage) {
    let input = &coverage.input;
    egui::Grid::new("root_terminal_binding")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(
                ui,
                "Input coverage",
                "All states matching the original input binding",
            );
            match &input.snapshot {
                RootTerminalSnapshot::Offline(snapshot) => {
                    value::text(ui, "Offline snapshot", &snapshot.label);
                    value::text(ui, "Offline namespace", snapshot.namespace.0.to_string());
                }
                RootTerminalSnapshot::Chain(snapshot) => {
                    value::text(ui, "Snapshot chain ID", &snapshot.chain_id);
                    value::text(ui, "Snapshot block hash", &snapshot.block_hash);
                }
            }
            value::text(ui, "Fact revision", input.revision.0.to_string());
            value::text(
                ui,
                "Storage source expression",
                WideSymbol::Expression(input.storage_source.node as u64).to_string(),
            );
            value::text(
                ui,
                "Original input scope",
                input
                    .input_scope
                    .map_or("None".into(), |scope| scope.0.to_string()),
            );
            value::text(ui, "Fork", format!("{:?}", input.fork));
            value::text(
                ui,
                "Selected numeric model",
                match input.numeric_model {
                    evm_abstract_protocol::NumericModel::Product => "Product",
                    evm_abstract_protocol::NumericModel::FiniteSet => "Finite set",
                },
            );
            value::text(ui, "Entry storage owner", &input.entry_address);
            value::text(ui, "Physical storage owner", &input.frame.address);
            value::text(ui, "Original code account", &input.frame.code_address);
            value::text(ui, "Original code hash", &input.frame.code_hash);
            value::text(
                ui,
                "Logical ADDRESS",
                value::address(&input.frame.address_value),
            );
            value::text(ui, "CALLER", value::address(&input.frame.caller));
            value::named(ui, "Original CALLVALUE", &input.call_value);
            value::text(
                ui,
                "Original static mode",
                input.frame.is_static.to_string(),
            );
            value::text(ui, "Original frame mode", format!("{:?}", input.frame.mode));
            value::text(
                ui,
                "Original basic block",
                input.frame.basic_block_index.to_string(),
            );
            value::text(
                ui,
                "Original stack height",
                input.frame.stack_height.to_string(),
            );
            value::text(
                ui,
                "Original jump history",
                format!("{:?}", input.frame.jump_history),
            );
            value::text(
                ui,
                "Captured program",
                Symbol::Program(input.program).to_string(),
            );
            value::text(ui, "Detailed boundary", format!("{:?}", coverage.trigger));
        });
    symbolic::show(
        ui,
        report,
        input.storage_source.node,
        "Original storage source",
    );
    ui.collapsing("Captured executing program", |ui| {
        if let Some(program) = report.programs.iter().find(|program| program.id == input.program) {
            ui.label("These bytes identify the original executing frame. Store inspection shows current account code separately.");
            egui::Grid::new("root_terminal_program").num_columns(2).show(ui, |ui| {
                value::text(ui, "Program", Symbol::Program(program.id).to_string());
                value::text(ui, "Code account", &program.code_address);
                value::text(ui, "Code hash", &program.code_hash);
                value::text(ui, "Code kind", format!("{:?}", program.kind));
                value::text(ui, "Captured bytecode", &program.bytecode);
            });
        }
    });
    original_inputs(ui, report, input);
    ui.collapsing("Original execution configuration", |ui| {
        config(ui, &input.config)
    });
    ui.collapsing("Original domain policy", |ui| {
        domain(ui, &input.domain_spec)
    });
    for (label, id) in [
        ("Saved store", input.saved_store),
        ("Initial store", input.initial_store),
    ] {
        let store_label = notation::widget(ui, format!("{label} {}", Symbol::Store(id)));
        ui.collapsing(store_label, |ui| {
            if let Some(store) = report.stores.get(id) {
                stores::changes(ui, report, store);
            }
        });
    }
}

fn original_inputs(ui: &mut Ui, report: &AnalysisReport, input: &RootTerminalInputBinding) {
    ui.collapsing("Original calldata", |ui| {
        ui.allocate_ui(egui::vec2(ui.available_width(), 140.0), |ui| {
            buffers::show(ui, report, input.calldata, "Original root calldata");
        });
    });
    ui.collapsing("Original effective execution environment", |ui| {
        context::effective_environment(ui, &input.environment, &report.metadata.fingerprint);
    });
    ui.label(
        RichText::new("Displayed input values may be less precise than the original binding retained by the analysis.")
            .small()
            .color(palette::MUTED),
    );
}

fn config(ui: &mut Ui, config: &RootTerminalConfig) {
    egui::Grid::new("root_terminal_config")
        .num_columns(2)
        .show(ui, |ui| {
            for (label, number) in [
                ("Jump context depth", config.context_depth),
                ("States", config.max_states),
                ("Transfers", config.max_transfers),
                ("Logical work", config.max_work),
                ("Call depth", config.max_call_depth),
                ("Memory bytes", config.max_memory_bytes),
            ] {
                value::text(ui, label, number.to_string());
            }
            value::text(
                ui,
                "Reuse complete callee summaries",
                config.use_summaries.to_string(),
            );
            value::text(
                ui,
                "Prepare conservative root terminal coverage",
                config.root_terminal_coverage.to_string(),
            );
        });
}

fn domain(ui: &mut Ui, domain: &RootTerminalDomainSpec) {
    egui::Grid::new("root_terminal_domain")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(ui, "Domain schema", domain.schema_version.to_string());
            value::text(ui, "Word bits", domain.word_bits.to_string());
            for (label, number) in [
                ("Constant capacity", domain.capacity),
                ("Reduction rounds", domain.reduction_rounds),
                ("Scalar fact limit", domain.fact_limit),
                ("Widening after updates", domain.widening_after_updates),
            ] {
                value::text(ui, label, number.to_string());
            }
            value::text(ui, "Logical cost version", domain.cost_version.to_string());
            value::text(ui, "Provenance policy", &domain.provenance_policy);
            relations(ui, &domain.relations);
        });
}

fn relations(ui: &mut Ui, limits: &evm_abstract_protocol::RootTerminalRelationLimits) {
    value::text(ui, "Local relations enabled", limits.enabled.to_string());
    value::text(
        ui,
        "Relational constraints",
        limits.max_constraints.to_string(),
    );
    value::text(ui, "Expression nodes", limits.max_nodes.to_string());
    value::text(ui, "Expression depth", limits.max_depth.to_string());
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
