//! Conservative inspection keeps original observations, joined procedure
//! inputs and paired relative outcomes in explicit, independent scopes.

mod bytes;
mod original;
mod stores;

use super::{
    InspectionAction, InspectionTarget, Inspector, Tab, process, scroll, summaries, value,
};
use crate::{
    app::minimum::{self, Selection},
    controls::{self, Density},
    palette,
};
use bytes::show as bytes;
use egui::{RichText, Ui};
use evm_abstract_protocol::{self as api, MinimumModel};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Scope {
    #[default]
    Original,
    Input,
    Outcome,
}

impl Scope {
    const ALL: [(Self, &'static str); 3] = [
        (Self::Original, "Original observations"),
        (Self::Input, "Procedure input"),
        (Self::Outcome, "Joint outcome"),
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Original => "Original observations",
            Self::Input => "Procedure input",
            Self::Outcome => "Joint outcome",
        }
    }
}

impl Inspector {
    pub(in crate::app) fn select_original(&mut self) {
        self.scope = Scope::Original;
        self.tab = Tab::Frame;
    }

    pub(in crate::app) fn minimum_summary(&self, selection: &Selection) -> String {
        format!(
            "Inspector{}: {} · {}{}{}",
            if self.expanded { "" } else { " hidden" },
            self.tab.label(),
            self.scope.label(),
            if self.panel.maximized() {
                " · Maximized"
            } else {
                ""
            },
            if selection.output.is_some() {
                " · outcome selected"
            } else {
                ""
            },
        )
    }

    pub(in crate::app) fn show_minimum(
        &mut self,
        ui: &mut Ui,
        report: &api::AnalysisReport,
        model: &MinimumModel,
        selected: &mut Selection,
    ) {
        if let Some(InspectionAction::Global(next)) = self.inspect(
            ui,
            InspectionTarget::Global {
                report,
                model,
                selection: *selected,
            },
        ) {
            *selected = next;
        }
    }

    pub(super) fn minimum_contents(
        &mut self,
        ui: &mut Ui,
        report: &api::AnalysisReport,
        model: &MinimumModel,
        selected: &mut Selection,
    ) {
        // Remembering the last rendered coordinates only chooses a useful
        // observation scope; navigation and selection remain owned by the shell.
        if selected.account.is_some()
            && self
                .last_minimum
                .is_none_or(|last| last.account != selected.account)
        {
            self.select_original();
        } else if self.last_minimum.is_some_and(|last| {
            (last.summary, last.procedure, last.node, last.control)
                != (
                    selected.summary,
                    selected.procedure,
                    selected.node,
                    selected.control,
                )
        }) {
            self.scope = Scope::Input;
        }
        if selected.output.is_some()
            && self.last_minimum.is_none_or(|last| {
                last.output != selected.output
                    || last.procedure != selected.procedure
                    || last.summary != selected.summary
            })
        {
            self.scope = Scope::Outcome;
        }
        self.navigation(ui);
        if self.last_minimum_tab != Some(self.tab) && self.tab == Tab::Outcomes {
            self.scope = Scope::Outcome;
        }
        self.last_minimum_tab = Some(self.tab);
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.available_width() < 600.0 {
                    egui::ComboBox::from_id_salt("inspector_observation_scope")
                        .width(158.0)
                        .selected_text(self.scope.label())
                        .show_ui(ui, |ui| {
                            for (scope, label) in Scope::ALL {
                                controls::tab_value(ui, &mut self.scope, scope, label, Density::Compact);
                            }
                        });
                } else {
                    for (scope, label) in Scope::ALL {
                        controls::tab_value(ui, &mut self.scope, scope, label, Density::Compact);
                    }
                }
                ui.label(RichText::new(if model.current { "Global MAY" } else { "Historical Global MAY" }).small().color(if model.current { palette::MUTED } else { palette::WARNING }))
                    .on_hover_text("Possible control and procedure bounds are qualified by this retained model. They do not supply a Native instruction stack or SSA value.");
            });
        });
        ui.separator();
        scroll(ui, "minimum_inspection", |ui| {
            if self.tab == Tab::Timing {
                super::super::timing::show(ui, report, &self.timings);
                return;
            }
            if self.tab == Tab::Acquisition {
                super::evidence::acquisition_rows(
                    ui,
                    report.acquisition.as_ref(),
                    model.accounts.len(),
                );
                return;
            }
            if self.tab == Tab::Environment {
                original::show(ui, report, model, selected);
                return;
            }
            if self.scope == Scope::Original
                && !matches!(self.tab, Tab::Coverage | Tab::Diagnostics)
            {
                match self.tab {
                    Tab::Calldata => bytes(ui, &model.environment.calldata),
                    Tab::Frame | Tab::Storage => original::show(ui, report, model, selected),
                    Tab::Transient => {
                        ui.label("Original transaction transient storage starts at zero. Procedure inputs and outcomes retain transaction-local changes.");
                    }
                    _ => {
                        ui.label("This field has no original observation. Select Procedure input, Joint outcome or a retained Native block snapshot.");
                    }
                }
                return;
            }
            let Some(summary) = selected.summary(model) else {
                ui.label("No procedure relation is attached to this source selection.");
                return;
            };
            let status = minimum::source_status(model, selected.summary);
            ui.label(
                RichText::new(summaries::coverage_label(summary.relation.closed, status))
                    .small()
                    .color(if status == api::SummarySourceStatus::Current {
                        palette::MUTED
                    } else {
                        palette::WARNING
                    }),
            );
            let mut procedure = selected.procedure;
            egui::ComboBox::from_id_salt("minimum_procedure")
                .width(ui.available_width().min(440.0))
                .selected_text(
                    summary
                        .relation
                        .procedures
                        .get(procedure)
                        .map(context_label)
                        .unwrap_or_else(|| "No compatible context".into()),
                )
                .show_ui(ui, |ui| {
                    for (id, relation) in summary.relation.procedures.iter().enumerate() {
                        ui.selectable_value(
                            &mut procedure,
                            id,
                            format!(
                                "{}{}",
                                context_label(relation),
                                if Some(id) == summary.relation.root_procedure {
                                    " · entry"
                                } else {
                                    ""
                                }
                            ),
                        );
                    }
                });
            if procedure != selected.procedure {
                selected.procedure = procedure;
                selected.output = None;
                selected.control = None;
                selected.account = None;
                self.scope = Scope::Input;
                selected.node = summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.procedure == procedure)
                    .map(|node| node.id);
            }
            let Some(relation) = summary.relation.procedures.get(selected.procedure) else {
                ui.label("This context is unavailable in the selected model. Choose a compatible context above.");
                return;
            };
            let node = selected.node.and_then(|id| {
                summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.id == id && node.procedure == selected.procedure)
            });
            if selected.node.is_some() && node.is_none() {
                ui.label("The selected regional observation does not belong to this context. Choose a compatible context above.");
                return;
            }
            if let Some(location) = selected.control {
                let admitted = node.is_some_and(|node| {
                    node.frame
                        .process
                        .as_ref()
                        .is_some_and(|frame| frame.location == location)
                        || node
                            .transfer
                            .as_ref()
                            .and_then(|transfer| transfer.process.as_ref())
                            .and_then(|process| process.control.as_ref())
                            .is_some_and(|region| {
                                region.entry == location
                                    || region.through.contains(&location)
                                    || region.boundaries.contains(&location)
                            })
                });
                if !admitted {
                    ui.label("The selected MAY location is not attached to this regional observation. Choose a source or context from the workspace.");
                    return;
                }
            }
            ui.label(if relation.is_static {
                "Context · read only"
            } else {
                "Context · writes permitted"
            });
            if self.tab == Tab::Frame {
                if self.scope == Scope::Input
                    && let Some(input) = &relation.process_input
                {
                    super::observation::process_input(ui, input);
                }
                ui.collapsing("Context and code source", |ui| {
                    if let Some(frame) = &relation.process {
                        process::frame(ui, frame);
                    } else {
                        ui.label(process::interpretation(relation.semantics));
                        if let Some(source) = &relation.code_address {
                            ui.monospace(format!("Code: {source}"));
                        }
                        if let Some(owner) = &relation.storage_address {
                            ui.monospace(format!("Storage owner: {owner}"));
                        }
                        ui.label(format!("Static: {}", relation.is_static));
                    }
                });
            }
            if matches!(self.tab, Tab::Coverage | Tab::Diagnostics) {
                if let Some(node) = node {
                    ui.label(RichText::new(crate::report::regional_status(node, status)).strong());
                    process::control(ui, report, node, selected.control);
                    ui.collapsing("Advanced regional receipt", |ui| {
                        ui.label(format!(
                            "Regional node r{} · procedure {}",
                            node.id, node.procedure
                        ));
                        summaries::regional::node_details(ui, node, status);
                    });
                }
                summaries::regional::show(ui, &summary.relation.regional_cfg, status);
                return;
            }
            let Some(bounds) = summary.bounds.get(selected.procedure) else {
                ui.label("Numerical bounds are not attached for this procedure.");
                return;
            };
            ui.collapsing("Advanced model and observation identity", |ui| {
                ui.label(format!(
                    "Procedure {} · evidence namespace {} · generation {}",
                    selected.procedure, summary.version.namespace, summary.version.generation
                ));
                ui.label(format!(
                    "Producer namespace {} · input revision {} · output revision {}",
                    bounds.version.namespace,
                    bounds.version.entry_revision,
                    bounds.version.output_revision
                ));
                if let Some(epoch) = &bounds.version.source_epoch {
                    ui.monospace(format!("Source epoch: {epoch}"));
                }
                ui.monospace(&model.fingerprint);
            });
            if matches!(
                self.tab,
                Tab::Stack | Tab::SsaValue | Tab::Memory | Tab::Calldata
            ) {
                ui.label(match self.tab {
                    Tab::Stack => "Conservative procedure bounds have no Native instruction stack. Select a retained Native block to inspect its stack.",
                    Tab::SsaValue => "Global MAY control has no Native SSA definition. Select a retained Native SSA value.",
                    Tab::Memory => "Conservative procedure bounds have no Native memory snapshot. Select a retained Native block before or after observation.",
                    _ => "Procedure calldata is not retained as a separate numerical byte array. Original calldata is available under Original observations.",
                });
                return;
            }
            if self.scope == Scope::Input {
                if matches!(self.tab, Tab::Returndata | Tab::Outcomes) {
                    ui.label("Return/revert data belongs to a Joint outcome; choose that scope to keep its data and store paired.");
                } else {
                    ui.label("Joined procedure input bounds");
                    selected_store(ui, &bounds.input, self.tab);
                }
                return;
            }
            if bounds.outputs.is_empty() {
                ui.label("No relative output alternative is attached.");
                return;
            }
            let mut output = selected.output.unwrap_or(0);
            if output >= bounds.outputs.len() {
                ui.label("This outcome is unavailable in the selected context. Select a current outcome from the graph.");
                return;
            }
            egui::ComboBox::from_id_salt("minimum_joint_output")
                .selected_text(format!(
                    "{:?} · alternative {} / {}",
                    bounds.outputs[output].kind,
                    output + 1,
                    bounds.outputs.len()
                ))
                .show_ui(ui, |ui| {
                    for (id, item) in bounds.outputs.iter().enumerate() {
                        if ui
                            .selectable_value(
                                &mut output,
                                id,
                                format!("{:?} · alternative {}", item.kind, id + 1),
                            )
                            .clicked()
                        {
                            selected.output = Some(id);
                        }
                    }
                });
            let output = &bounds.outputs[output];
            ui.label("Data and Store below belong to this same relative outcome. The caller applies rollback.");
            ui.collapsing("Advanced outcome producer", |ui| {
                ui.label(format!("Output generation {}", output.generation));
            });
            if self.tab == Tab::Returndata {
                bytes(ui, &output.data);
            } else {
                ui.collapsing("Returned data", |ui| bytes(ui, &output.data));
                selected_store(ui, &output.store, self.tab);
            }
        });
        self.last_minimum = Some(*selected);
    }
}

fn context_label(relation: &api::AbstractProcedure) -> String {
    let source = relation
        .process
        .as_ref()
        .map(|frame| process::source_label(&frame.source))
        .unwrap_or_else(|| {
            relation
                .code_address
                .clone()
                .unwrap_or_else(|| format!("{:?}", relation.kind))
        });
    let owner = relation
        .process
        .as_ref()
        .map(|frame| process::owner(&frame.owner))
        .or_else(|| relation.storage_address.clone())
        .unwrap_or_else(|| "Unknown owner".into());
    format!(
        "{source} · {owner}{}",
        if relation.is_static { " · static" } else { "" }
    )
}

fn selected_store(ui: &mut Ui, store: &api::MinimumStore, tab: Tab) {
    match tab {
        Tab::Storage => stores::plane(ui, "minimum_persistent", &store.persistent),
        Tab::Transient => stores::plane(ui, "minimum_transient", &store.transient),
        _ => stores::show(ui, store),
    }
}
