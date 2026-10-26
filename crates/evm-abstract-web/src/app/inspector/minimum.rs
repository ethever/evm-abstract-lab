//! Read-only numerical observations retain their procedure and joint output
//! association. Original World observations stay separate from joined inputs.

use super::{Inspector, Tab, process, scroll, summaries, value};
use crate::{
    app::minimum::{self, Selection},
    controls::{self, Density},
};
use egui::{RichText, Ui};
use evm_abstract_protocol::{
    self as api, MinimumByteArray, MinimumByteContent, MinimumModel, MinimumPlane, MinimumStore,
};

impl Inspector {
    pub(in crate::app) fn minimum_summary(&self, selection: &Selection) -> String {
        let tab = match self.tab {
            Tab::Coverage => "Control",
            Tab::Outcomes | Tab::Returndata => "Joint outputs",
            Tab::Storage | Tab::Transient => "Procedure input",
            Tab::Timing => "Timing",
            _ => "Original inputs",
        };
        format!(
            "Inspector{}: {tab}{} · procedure {} · output {:?}",
            if self.expanded { "" } else { " hidden" },
            if self.panel.maximized() {
                " · Maximized"
            } else {
                ""
            },
            selection.procedure,
            selection.output
        )
    }

    pub(in crate::app) fn show_minimum(
        &mut self,
        ui: &mut Ui,
        report: &api::AnalysisReport,
        model: &MinimumModel,
        selected: &mut Selection,
    ) {
        let rect = ui.available_rect_before_wrap();
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id(egui::Id::new("inspector_contents"))
                .max_rect(rect),
        );
        child.set_clip_rect(child.clip_rect().intersect(rect));
        let ui = &mut child;
        let mut tab = match self.tab {
            Tab::Coverage => Tab::Coverage,
            Tab::Outcomes | Tab::Returndata => Tab::Outcomes,
            Tab::Storage | Tab::Transient => Tab::Storage,
            Tab::Timing => Tab::Timing,
            _ => Tab::Frame,
        };
        let previous = tab;
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (choice, label) in [
                    (Tab::Frame, "Original inputs"),
                    (Tab::Coverage, "Control"),
                    (Tab::Storage, "Procedure input"),
                    (Tab::Outcomes, "Joint outputs"),
                    (Tab::Timing, "Timing"),
                ] {
                    controls::tab_value(ui, &mut tab, choice, label, Density::Compact);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.window_controls(ui)
                });
            })
        });
        if tab != previous {
            self.tab = tab;
        }
        scroll(ui, "minimum_inspection", |ui| {
            if tab == Tab::Timing {
                super::super::timing::show(ui, report, &self.timings);
                return;
            }
            if tab == Tab::Frame {
                original_inputs(ui, model, selected);
                return;
            }
            let Some(summary) = selected.summary(model) else {
                ui.label("No procedure relation is attached.");
                return;
            };
            let source = minimum::source_status(model, selected.summary);
            ui.label(summaries::coverage_label(summary.relation.closed, source));
            ui.label("Numerical observations belong to this qualified model and procedure.");
            let mut procedure = selected.procedure;
            egui::ComboBox::from_id_salt("minimum_procedure")
                .selected_text(format!("Procedure {procedure}"))
                .show_ui(ui, |ui| {
                    for (id, relation) in summary.relation.procedures.iter().enumerate() {
                        ui.selectable_value(
                            &mut procedure,
                            id,
                            format!(
                                "Procedure {id} · {:?}{}",
                                relation.kind,
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
                selected.node = summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.procedure == procedure)
                    .map(|node| node.id);
            }
            if let Some(relation) = summary.relation.procedures.get(selected.procedure) {
                if let Some(frame) = &relation.process {
                    process::frame(ui, frame);
                }
                ui.label(format!(
                    "Interpretation: {:?} · static: {}",
                    relation.semantics, relation.is_static
                ));
            }
            if tab == Tab::Coverage {
                if let Some(node) = selected.node.and_then(|id| {
                    summary
                        .relation
                        .regional_cfg
                        .nodes
                        .iter()
                        .find(|node| node.id == id)
                }) {
                    ui.label(RichText::new(format!("Regional node r{}", node.id)).strong());
                    ui.label(crate::report::regional_status(node, source));
                    summaries::regional::node_details(ui, node, source);
                }
                summaries::regional::show(ui, &summary.relation.regional_cfg, source);
                return;
            }
            let Some(bounds) = summary.bounds.get(selected.procedure) else {
                ui.label("Numerical bounds are not attached for this procedure.");
                return;
            };
            ui.label(format!(
                "Producer namespace {} · input revision {} · output revision {}",
                bounds.version.namespace,
                bounds.version.entry_revision,
                bounds.version.output_revision
            ));
            if tab == Tab::Storage {
                ui.label("Joined procedure input bounds");
                store(ui, &bounds.input);
                return;
            }
            if bounds.outputs.is_empty() {
                ui.label("No relative output alternative is attached.");
                return;
            }
            let mut output = selected.output.unwrap_or(0).min(bounds.outputs.len() - 1);
            egui::ComboBox::from_id_salt("minimum_joint_output")
                .selected_text(format!(
                    "{:?} · generation {}",
                    bounds.outputs[output].kind, bounds.outputs[output].generation
                ))
                .show_ui(ui, |ui| {
                    for (id, item) in bounds.outputs.iter().enumerate() {
                        ui.selectable_value(
                            &mut output,
                            id,
                            format!("{:?} · generation {}", item.kind, item.generation),
                        );
                    }
                });
            selected.output = Some(output);
            let output = &bounds.outputs[output];
            ui.label("Data and Store below belong to this same relative outcome. The caller applies rollback.");
            ui.collapsing("Returned data", |ui| bytes(ui, &output.data));
            store(ui, &output.store);
        });
        ui.advance_cursor_after_rect(rect);
    }
}

fn original_inputs(ui: &mut Ui, model: &MinimumModel, selected: &mut Selection) {
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
    ui.collapsing("Original accounts and root template",|ui| {
        ui.label(format!("{} supplied rows",model.accounts.len()));
        if model.accounts.is_empty() {ui.label("No supplied account row is attached.");return;}
        let mut chosen=selected.account.unwrap_or(0).min(model.accounts.len()-1);
        egui::ComboBox::from_id_salt("minimum_input_account").selected_text(&model.accounts[chosen].address).show_ui(ui,|ui| {
            for (id,account) in model.accounts.iter().enumerate(){ui.selectable_value(&mut chosen,id,&account.address);}
        });
        selected.account=Some(chosen);
        let account=&model.accounts[chosen];
            ui.push_id(&account.address,|ui|egui::CollapsingHeader::new(format!("{} · {}",account.address,if account.root_template {"Root input template"} else {"Physical account"})).default_open(true).show(ui,|ui| {
                if account.root_template { ui.label("This supplied row is an input namespace; logical ADDRESS retains the original symbolic qualification."); }
                egui::Grid::new("minimum_original_account").num_columns(2).show(ui,|ui| {
                    value::text(ui,"Existence",format!("{:?}",account.existence));
                    value::named(ui,"Balance",&account.balance);
                    value::named(ui,"Nonce",&account.nonce);
                    value::text(ui,"Code",format!("{:?} · program {:?}",account.code_kind,account.program));
                    if let Some(target)=&account.delegation_target { value::text(ui,"Delegation target",target); }
                    value::text(ui,"Unlisted initial slots",if account.storage_unknown {"Unknown"} else {"Zero"});
                });
                if !account.storage.is_empty() {
                    original_slots(ui,&account.storage);
                }
            }));

    });
    ui.collapsing("Model qualification and policy", |ui| {
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

fn bytes(ui: &mut Ui, data: &MinimumByteArray) {
    egui::Grid::new("minimum_bytes_length")
        .num_columns(2)
        .show(ui, |ui| {
            value::named(ui, "Length", &data.length);
            value::text(ui, "Memory rules", data.memory.to_string());
        });
    match &data.content {
        MinimumByteContent::Zero => {
            ui.label("All content bytes are zero.");
        }
        MinimumByteContent::Exact(exact) => {
            ui.monospace(if exact.len() > 130 {
                format!(
                    "{}… ({} hex digits)",
                    exact.chars().take(130).collect::<String>(),
                    exact.len()
                )
            } else {
                exact.clone()
            });
            if controls::action(ui, "Copy complete bytes", Density::Compact).clicked() {
                ui.ctx().copy_text(exact.clone());
            }
        }
        MinimumByteContent::Cells(cells) => {
            ui.label("Unlisted byte cells are unconstrained.");
            byte_cells(ui, "minimum_byte_cells", cells);
        }
    }
    if !data.words.is_empty() {
        ui.label("Additional whole-word bounds");
        byte_cells(ui, "minimum_word_cells", &data.words);
    }
}

fn byte_cells(ui: &mut Ui, id: &str, cells: &[api::MinimumByteCell]) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(220.0)
        .show_rows(ui, 22.0, cells.len(), |ui, rows| {
            for row in rows {
                ui.horizontal(|ui| {
                    ui.monospace(format!("0x{:x}", cells[row].offset));
                    value::cell(ui, &cells[row].value);
                });
            }
        });
}

fn store(ui: &mut Ui, store: &MinimumStore) {
    ui.label("Physical owner/alias bounds");
    ui.collapsing("Persistent storage", |ui| {
        plane(ui, "minimum_persistent", &store.persistent)
    });
    ui.collapsing("Transient storage", |ui| {
        plane(ui, "minimum_transient", &store.transient)
    });
    ui.collapsing("Balances and nonces", |ui| {
        ui.label("Unlisted balance bound");
        value::details(ui, &store.balance_default);
        owner_values(ui, "minimum_balances", &store.balances);
        ui.label("Unlisted nonces are unconstrained words.");
        owner_values(ui, "minimum_nonces", &store.nonces);
    });
    ui.collapsing("Code and presence", |ui| {
        ui.label("Unlisted code and account presence are unknown.");
        for code in &store.codes {
            ui.monospace(format!(
                "{} · {:?} · program {:?} · delegation {:?}",
                code.owner, code.kind, code.program, code.delegation_target
            ));
        }
        for presence in &store.existence {
            ui.monospace(format!("{} · {:?}", presence.owner, presence.value));
        }
    });
    ui.collapsing("Lifecycle bounds", |ui| {
        flags(ui, "Created", store.created_default, &store.created);
        flags(
            ui,
            "Pending destruction",
            store.pending_destruction_default,
            &store.pending_destruction,
        );
        flags(ui, "Touched", store.touched_default, &store.touched);
    });
    ui.collapsing("Possible log sources", |ui| {
        ui.label(format!(
            "{} source alternatives · additional sources: {}",
            store.logs.len(),
            store.logs_unknown
        ));
        ui.label("Source alternatives do not preserve event order or multiplicity.");
        for (id, log) in store.logs.iter().enumerate() {
            ui.push_id(id, |ui| {
                ui.collapsing(format!("{} · PC 0x{:x}", log.address, log.pc), |ui| {
                    ui.monospace(format!(
                        "Code {} · {} · {:?}",
                        log.code_address, log.code_hash, log.source_kind
                    ));
                    for (id, topic) in log.topics.iter().enumerate() {
                        ui.label(format!("Topic {id}"));
                        value::details(ui, topic);
                    }
                    bytes(ui, &log.data);
                })
            });
        }
    });
}

fn owner_values(ui: &mut Ui, id: &str, rows: &[api::MinimumOwnerValue]) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(220.0)
        .show_rows(ui, 22.0, rows.len(), |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.monospace(&rows[row].owner);
                    value::cell(ui, &rows[row].value);
                });
            }
        });
}

fn flags(ui: &mut Ui, label: &str, default: Option<bool>, rows: &[api::MinimumOwnerFlag]) {
    let text = |value: Option<bool>| {
        value.map_or("Both allowed", |value| if value { "True" } else { "False" })
    };
    ui.label(format!("{label}: unlisted owners {}", text(default)));
    for row in rows {
        ui.monospace(format!("{} · {}", row.owner, text(row.value)));
    }
}

fn plane(ui: &mut Ui, id: &str, plane: &MinimumPlane) {
    ui.label("Unlisted owner/slot bound");
    value::details(ui, &plane.default);
    ui.label("Owner defaults");
    owner_values(ui, &format!("{id}_owners"), &plane.owners);
    ui.label("Explicit physical slots");
    let table = ui.id().with(egui::IdSalt::new(id));
    let width = super::stores::slots::default_width(ui);
    let reset = super::stores::slots::refresh_default(ui, table, width);
    let builder = egui_extras::TableBuilder::new(ui)
        .id_salt(id)
        .resizable(true)
        .vscroll(true)
        .max_scroll_height(260.0);
    if reset {
        builder.reset();
    }
    builder
        .column(egui_extras::Column::auto())
        .column(
            egui_extras::Column::initial(width)
                .clip(true)
                .resizable(true),
        )
        .column(egui_extras::Column::remainder().at_least(180.0))
        .header(22.0, |mut row| {
            row.col(|ui| {
                ui.label("Physical owner");
            });
            row.col(|ui| {
                ui.label("Slot");
            });
            row.col(|ui| {
                ui.label("Bound");
            });
        })
        .body(|body| {
            body.rows(22.0, plane.slots.len(), |mut row| {
                let entry = &plane.slots[row.index()];
                row.col(|ui| {
                    ui.monospace(&entry.owner);
                });
                row.col(|ui| super::stores::slots::cell(ui, &entry.slot));
                row.col(|ui| value::cell(ui, &entry.value));
            })
        });
    super::stores::slots::remember_resize(ui, table);
    ui.collapsing("Dependence on initial slot values", |ui| {
        ui.label(format!(
            "Unlisted coordinates independent: {}",
            plane.independent_default
        ));
        for owner in &plane.independent_owners {
            ui.monospace(format!("{} · independent {:?}", owner.owner, owner.value));
        }
        for slot in &plane.independent_slots {
            ui.monospace(format!(
                "{} · {} · independent {}",
                slot.owner, slot.slot, slot.independent
            ));
        }
    });
}

fn original_slots(ui: &mut Ui, entries: &[api::StorageEntry]) {
    let id = "minimum_original_slots";
    let table = ui.id().with(egui::IdSalt::new(id));
    let width = super::stores::slots::default_width(ui);
    let reset = super::stores::slots::refresh_default(ui, table, width);
    let builder = egui_extras::TableBuilder::new(ui)
        .id_salt(id)
        .resizable(true)
        .vscroll(true)
        .max_scroll_height(240.0);
    if reset {
        builder.reset();
    }
    builder
        .column(
            egui_extras::Column::initial(width)
                .clip(true)
                .resizable(true),
        )
        .column(egui_extras::Column::remainder().at_least(180.0))
        .header(22.0, |mut row| {
            row.col(|ui| {
                ui.label("Slot");
            });
            row.col(|ui| {
                ui.label("Original value");
            });
        })
        .body(|body| {
            body.rows(22.0, entries.len(), |mut row| {
                let entry = &entries[row.index()];
                row.col(|ui| super::stores::slots::cell(ui, &entry.slot));
                row.col(|ui| value::cell(ui, &entry.value));
            })
        });
    super::stores::slots::remember_resize(ui, table);
}
