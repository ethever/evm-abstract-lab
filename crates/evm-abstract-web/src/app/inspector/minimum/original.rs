//! Original World observations retain their input-template qualification.

use super::{bytes, value};
use crate::app::minimum::Selection;
use egui::Ui;
use evm_abstract_protocol::{self as api, MinimumModel};

pub(super) fn show(
    ui: &mut Ui,
    report: &api::AnalysisReport,
    model: &MinimumModel,
    selected: &mut Selection,
) {
    environment(ui, model);
    ui.collapsing("Original accounts and root template", |ui| {
        accounts(ui, model, selected);
    });
    ui.collapsing("Model qualification and policy", |ui| {
        if let Some(policy) = crate::app::hybrid::source_policy(report) {
            ui.label(policy);
        }
        ui.monospace(format!(
            "Source: {:?} · facts {}",
            model.source_snapshot, model.revision
        ));
        ui.monospace(&model.fingerprint);
        ui.label(format!(
            "{:?} · {:?} · current: {}",
            model.fork, model.numeric_model, model.current
        ));
        ui.monospace(format!("{:?}", model.domain_spec));
        ui.monospace(format!("{:?}", model.config));
        ui.label(format!(
            "Minimum projection work {} · application allowance {}",
            model.export_usage.work, model.postprocessing_max_work
        ));
    });
}

pub(in crate::app::inspector) fn environment(ui: &mut Ui, model: &MinimumModel) {
    ui.label("Original input observations");
    ui.label("These rows precede analysis effects; procedure inputs may join additional invocation states.");
    egui::Grid::new("minimum_original_environment")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(
                ui,
                "Original input scope",
                model
                    .environment
                    .input_scope
                    .map_or("Not retained".into(), |scope| {
                        evm_abstract_notation::WideSymbol::Scope(scope.0).to_string()
                    }),
            );
            value::text(ui, "Logical ADDRESS", value::address(&model.environment.to));
            value::text(ui, "CALLER", value::address(&model.environment.caller));
            value::text(ui, "ORIGIN", value::address(&model.environment.origin));
            value::named(ui, "CALLVALUE", &model.environment.call_value);
            value::text(ui, "Static", model.environment.is_static.to_string());
            value::text(
                ui,
                "Gas upper bound",
                model
                    .environment
                    .gas_upper_bound
                    .as_deref()
                    .unwrap_or("Unknown"),
            );
            for (name, bound) in [
                ("Gas price", &model.environment.gas_price),
                ("Timestamp", &model.environment.timestamp),
                ("Block number", &model.environment.number),
                ("Randomness", &model.environment.prevrandao),
                ("Block gas limit", &model.environment.gas_limit),
                ("Chain ID", &model.environment.chain_id),
                ("Base fee", &model.environment.base_fee),
                ("Blob base fee", &model.environment.blob_base_fee),
                ("Blob count", &model.environment.blob_count),
            ] {
                value::named(ui, name, bound);
            }
            value::text(ui, "Coinbase", value::address(&model.environment.coinbase));
        });
    ui.collapsing("Original calldata", |ui| {
        bytes(ui, &model.environment.calldata)
    });
}

fn accounts(ui: &mut Ui, model: &MinimumModel, selected: &mut Selection) {
    ui.label(format!("{} supplied rows", model.accounts.len()));
    if model.accounts.is_empty() {
        ui.label("No supplied account row is attached.");
        return;
    }
    let mut chosen = selected.account.unwrap_or(0);
    if chosen >= model.accounts.len() {
        ui.label(
            "The selected account is unavailable in this model. Choose an account from Sources.",
        );
        return;
    }
    egui::ComboBox::from_id_salt("minimum_input_account")
        .width(ui.available_width().min(360.0))
        .selected_text(&model.accounts[chosen].address)
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().show_rows(ui, 22.0, model.accounts.len(), |ui, rows| {
                for id in rows {
                    if ui
                        .selectable_value(&mut chosen, id, &model.accounts[id].address)
                        .clicked()
                    {
                        selected.account = Some(id);
                    }
                }
            });
        });
    account(ui, &model.accounts[chosen]);
}

pub(in crate::app::inspector) fn account(ui: &mut Ui, account: &api::MinimumInputAccount) {
    ui.push_id(&account.address, |ui| {
        egui::CollapsingHeader::new(format!(
            "{} · {}", account.address,
            if account.root_template { "Root input template" } else { "Physical account" }
        ))
        .default_open(true)
        .show(ui, |ui| {
            if account.root_template {
                ui.label("This supplied row is an input namespace; logical ADDRESS retains the original symbolic qualification.");
            }
            egui::Grid::new("minimum_original_account").num_columns(2).show(ui, |ui| {
                value::text(ui, "Existence", format!("{:?}", account.existence));
                value::named(ui, "Balance", &account.balance);
                value::named(ui, "Nonce", &account.nonce);
                value::text(ui, "Code", format!("{:?}", account.code_kind));
                if let Some(target) = &account.delegation_target {
                    value::text(ui, "Delegation target", target);
                }
                value::text(ui, "Unlisted initial slots", if account.storage_unknown { "Unknown" } else { "Zero" });
            });
            ui.collapsing("Advanced account identity", |ui| {
                ui.monospace(format!("Captured program: {:?}", account.program));
            });
            if !account.storage.is_empty() {
                super::stores::original_slots(ui, &account.storage);
            }
        });
    });
}
