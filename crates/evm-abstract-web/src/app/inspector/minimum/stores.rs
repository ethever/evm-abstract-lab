//! Numerical stores remain qualified by one procedure input or joint output.

use super::super::stores::slots;
use super::{bytes, value};
use egui::Ui;
use evm_abstract_protocol::{self as api, MinimumPlane, MinimumStore};

pub(super) fn show(ui: &mut Ui, store: &MinimumStore) {
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

pub(super) fn plane(ui: &mut Ui, id: &str, plane: &MinimumPlane) {
    ui.label("Unlisted owner/slot bound");
    value::details(ui, &plane.default);
    ui.label("Owner defaults");
    owner_values(ui, &format!("{id}_owners"), &plane.owners);
    ui.label("Explicit physical slots");
    let table = ui.id().with(egui::IdSalt::new(id));
    let width = slots::default_width(ui);
    let reset = slots::refresh_default(ui, table, width);
    let builder = egui_extras::TableBuilder::new(ui)
        .id_salt(id)
        .resizable(true)
        .vscroll(false);
    if reset {
        builder.reset();
    }
    builder
        .column(
            egui_extras::Column::initial(168.0)
                .at_least(96.0)
                .clip(true),
        )
        .column(
            egui_extras::Column::initial(width)
                .at_least(56.0)
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
                row.col(|ui| slots::cell(ui, &entry.slot));
                row.col(|ui| value::cell(ui, &entry.value));
            })
        });
    slots::remember_resize_at(ui, table, 1);
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

pub(super) fn original_slots(ui: &mut Ui, entries: &[api::StorageEntry]) {
    let id = "minimum_original_slots";
    let table = ui.id().with(egui::IdSalt::new(id));
    let width = slots::default_width(ui);
    let reset = slots::refresh_default(ui, table, width);
    let builder = egui_extras::TableBuilder::new(ui)
        .id_salt(id)
        .resizable(true)
        .vscroll(false);
    if reset {
        builder.reset();
    }
    builder
        .column(
            egui_extras::Column::initial(width)
                .at_least(56.0)
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
                row.col(|ui| slots::cell(ui, &entry.slot));
                row.col(|ui| value::cell(ui, &entry.value));
            })
        });
    slots::remember_resize(ui, table);
}
