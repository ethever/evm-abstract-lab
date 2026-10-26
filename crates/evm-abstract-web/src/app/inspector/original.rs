//! Original inputs are report observations, not procedure or machine coordinates.

use super::{Inspector, OriginalAccount, Tab, context, minimum, scroll, value};
use egui::Ui;
use evm_abstract_protocol::{self as api, AnalysisReport, RootInputBinding};

enum Observation<'a> {
    Minimum(&'a api::MinimumModel, Option<&'a api::MinimumInputAccount>),
    Captured(&'a api::AnalysisGraph, Option<&'a api::AccountState>),
}

impl Inspector {
    pub(super) fn original_contents(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        account: OriginalAccount<'_>,
    ) {
        self.navigation(ui);
        ui.label("Original observations · no execution context selected");
        scroll(ui, "original_inspection", |ui| {
            // Resolve before rendering any facts, including environment facts.
            // An invalid pick must never fall back to the first supplied row.
            let Some(observation) = resolve(report, account) else {
                ui.label("This original account is unavailable in this report. Choose an original account from Sources.");
                return;
            };
            match self.tab {
                Tab::Timing => super::super::timing::show(ui, report, &self.timings),
                Tab::Acquisition => super::evidence::acquisition_rows(
                    ui,
                    report.acquisition.as_ref(),
                    match observation {
                        Observation::Minimum(model, _) => model.accounts.len(),
                        Observation::Captured(graph, _) => graph.accounts.len(),
                    },
                ),
                Tab::Frame | Tab::Storage | Tab::Environment => {
                    let root = matches!(account, OriginalAccount::Root);
                    match observation {
                        Observation::Minimum(model, row) => {
                            if self.tab == Tab::Environment
                                || (self.tab == Tab::Frame
                                    && (root || row.is_some_and(|row| row.root_template)))
                            {
                                minimum::original_environment(ui, model);
                            }
                            if let Some(row) = row {
                                minimum::original_account(ui, row);
                            } else {
                                ui.label("No supplied root account row is attached.");
                            }
                        }
                        Observation::Captured(graph, row) => {
                            if self.tab == Tab::Environment || (self.tab == Tab::Frame && root) {
                                ui.label("Declared root input");
                                super::process::source(ui, &graph.metadata.declared_root_source);
                                context::effective_environment(
                                    ui,
                                    &graph.metadata.environment,
                                    &graph.metadata.fingerprint,
                                );
                            }
                            if let Some(row) = row {
                                captured_account(ui, graph, row);
                            } else {
                                ui.label("No supplied root account row is attached.");
                            }
                        }
                    }
                }
                Tab::Calldata => match observation {
                    Observation::Minimum(model, _) => {
                        minimum::original_calldata(ui, &model.environment.calldata);
                    }
                    Observation::Captured(graph, _) => {
                        super::buffers::show(
                            ui,
                            graph,
                            graph.metadata.environment.calldata,
                            "Original calldata",
                        );
                    }
                },
                Tab::Transient => {
                    ui.label("Original transaction transient storage starts at zero. Procedure inputs and outcomes retain transaction-local changes.");
                }
                _ => {
                    ui.label("This field has no original observation. Choose a compatible context to inspect procedure inputs, joint outcomes or Native block snapshots.");
                }
            }
        });
    }
}

fn resolve<'a>(report: &'a AnalysisReport, target: OriginalAccount<'_>) -> Option<Observation<'a>> {
    if let OriginalAccount::Address(address) = target
        && !physical_address(address)
    {
        return None;
    }
    if let Some(model) = report.minimum.as_deref() {
        let row = match target {
            OriginalAccount::Root => {
                let (address, template) = root(&model.root_binding);
                model
                    .accounts
                    .iter()
                    .find(|row| row.address == address && row.root_template == template)
            }
            OriginalAccount::Minimum(index) => Some(model.accounts.get(index)?),
            OriginalAccount::Address(address) => Some(
                model
                    .accounts
                    .iter()
                    .find(|row| !row.root_template && row.address == address)?,
            ),
        };
        return Some(Observation::Minimum(model, row));
    }
    // Global retains the original symbolic root; Hybrid Native may bind that
    // namespace to a concrete ADDRESS. Only a standalone Native publication
    // supplies its own original input. Neither path selects a state or store.
    let graph = match report.global.as_deref() {
        Some(graph) => graph,
        None if report.hybrid.is_none() => report.native.as_deref()?,
        None => return None,
    };
    let row = match target {
        OriginalAccount::Root => {
            let (address, _) = root(&graph.metadata.root_binding);
            graph.accounts.iter().find(|row| row.address == address)
        }
        OriginalAccount::Minimum(_) => return None,
        OriginalAccount::Address(address) => Some(
            graph
                .accounts
                .iter()
                .find(|row| row.address == address && graph.input_namespace() != Some(address))?,
        ),
    };
    Some(Observation::Captured(graph, row))
}

fn physical_address(address: &str) -> bool {
    address.strip_prefix("0x").is_some_and(|digits| {
        digits.len() == 40 && digits.bytes().all(|digit| digit.is_ascii_hexdigit())
    })
}

fn root(binding: &RootInputBinding) -> (&str, bool) {
    match binding {
        RootInputBinding::Physical(root) => (&root.address, false),
        RootInputBinding::Namespace(root) => (&root.coordinate, true),
    }
}

fn captured_account(ui: &mut Ui, graph: &api::AnalysisGraph, row: &api::AccountState) {
    ui.push_id(&row.address, |ui| {
        let template = graph.input_namespace() == Some(row.address.as_str());
        ui.monospace(format!("{} · {}", row.address, if template { "Root input template" } else { "Physical account" }));
        if template {
            ui.label("This supplied row is an input namespace; logical ADDRESS retains the original symbolic qualification.");
        }
        egui::Grid::new("original_captured_account").num_columns(2).show(ui, |ui| {
            value::text(ui, "Existence", format!("{:?}", row.existence));
            value::named(ui, "Balance", &row.balance);
            value::named(ui, "Nonce", &row.nonce);
            value::text(ui, "Code", format!("{:?}", row.code_kind));
            value::text(ui, "Code hash", row.code_hash.as_deref().unwrap_or("Unknown"));
            if let Some(target) = &row.delegation_target {
                value::text(ui, "Delegation target", target);
            }
            value::named(ui, "Unlisted initial slots", &row.storage_default);
        });
        if !row.storage.is_empty() {
            minimum::original_slots(ui, &row.storage);
        }
    });
}
