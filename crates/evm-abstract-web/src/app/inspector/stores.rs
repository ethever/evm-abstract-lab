//! Persistent/transient storage and finalized state changes remain separate.
use crate::report::ReportView;

use super::value;
use crate::palette;
use egui::{RichText, Ui, UiBuilder};
use egui_extras::{Column, TableBuilder};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{AccountCode, AccountState, StorageEntry, StoreSnapshot, ValueInfo};

pub(super) mod slots;

pub(super) fn account<'a>(
    store: Option<&'a StoreSnapshot>,
    report: &'a ReportView<'_>,
    address: &str,
) -> Option<&'a AccountState> {
    match store {
        Some(store) => store
            .accounts
            .iter()
            .find(|account| account.address == address),
        None => report
            .accounts
            .iter()
            .find(|account| account.address == address),
    }
}

pub(super) fn storage(
    ui: &mut Ui,
    report: &ReportView<'_>,
    store: Option<&StoreSnapshot>,
    address: &str,
    transient: bool,
) {
    let account = account(store, report, address);
    egui::ScrollArea::both()
        .id_salt(("storage_slots", transient, address))
        .min_scrolled_height(0.0)
        .content_margin(egui::Margin::ZERO)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new(address).monospace());
            if let Some(store) = store {
                touch_default(ui, store);
                super::symbolic::facts(
                    ui,
                    if transient {
                        &store.transient_facts
                    } else {
                        &store.storage_facts
                    },
                    "Store-local scalar facts",
                );
            }
            if let Some(account) = account {
                let root = if transient {
                    account.transient_root
                } else {
                    account.storage_root
                };
                super::symbolic::show(ui, report, root.node, "Ordered storage state");
                egui::CollapsingHeader::new("Account facts in this store")
                    .id_salt(("storage_account_facts", address, transient))
                    .show(ui, |ui| {
                        egui::Grid::new(("storage_account_facts_grid", address, transient))
                            .num_columns(2)
                            .show(ui, |ui| {
                                value::text(ui, "Existence", format!("{:?}", account.existence));
                                value::named(ui, "Balance", &account.balance);
                                value::named(ui, "Nonce", &account.nonce);
                                value::text(ui, "Code", format!("{:?}", account.code_kind));
                                value::text(
                                    ui,
                                    "Code hash",
                                    account.code_hash.as_deref().unwrap_or("Unknown"),
                                );
                                if let Some(target) = &account.delegation_target {
                                    value::text(ui, "Delegation implementation", target);
                                }
                                if let Some(program) = account.program {
                                    value::text(
                                        ui,
                                        "Captured program",
                                        Symbol::Program(program).to_string(),
                                    );
                                }
                                value::text(
                                    ui,
                                    "Created in transaction",
                                    certainty(account.created),
                                );
                                value::text(
                                    ui,
                                    "Touched in transaction",
                                    touch_status(account.touched),
                                );
                                value::text(
                                    ui,
                                    "Pending destruction",
                                    certainty(account.pending_destruction),
                                );
                            });
                    });
            }
            let default = account
                .map(|account| {
                    if transient {
                        &account.transient_default
                    } else {
                        &account.storage_default
                    }
                })
                .or_else(|| {
                    store.map(|store| {
                        if transient {
                            &store.transient_default
                        } else {
                            &store.storage_default
                        }
                    })
                });
            if let Some(default) = default {
                ui.horizontal(|ui| {
                    ui.label("Base slot default");
                    value::cell(ui, default);
                });
            }
            let entries = account.map(|account| {
                if transient {
                    &account.transient
                } else {
                    &account.storage
                }
            });
            let Some(entries) = entries.filter(|entries| !entries.is_empty()) else {
                ui.label(
                    "No concrete slot observations. Ordered symbolic writes and source facts remain in the state graph above.",
                );
                return;
            };
            storage_rows(ui, entries, transient);
        });
}

/// Table widths are presentation preferences per storage kind, independent of
/// the account or selected observation. Rows use the outer scroll area's clip.
fn storage_rows(ui: &mut Ui, entries: &[StorageEntry], transient: bool) {
    let row_height = 21.0_f32
        .max(ui.text_style_height(&egui::TextStyle::Monospace))
        .max(ui.spacing().interact_size.y);
    ui.scope_builder(
        UiBuilder::new().id(egui::Id::new(("inspector_storage_slot_table", transient))),
        |ui| {
            let table_id = ui.id().with(egui::IdSalt::new("slots"));
            let default_width = slots::default_width(ui);
            let reset_default = slots::refresh_default(ui, table_id, default_width);
            let table = TableBuilder::new(ui)
                .id_salt("slots")
                .vscroll(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(
                    Column::initial(default_width)
                        .at_least(56.0)
                        .clip(true)
                        .resizable(true),
                )
                .column(Column::remainder().at_least(96.0).clip(true));
            if reset_default {
                table.reset();
            }
            table
                .header(row_height, |mut header| {
                    header.col(|ui| {
                        ui.strong("Slot");
                    });
                    header.col(|ui| {
                        ui.strong("Value");
                    });
                })
                .body(|body| {
                    body.rows(row_height, entries.len(), |mut row| {
                        let entry = &entries[row.index()];
                        row.col(|ui| slots::cell(ui, &entry.slot));
                        row.col(|ui| value::cell(ui, &entry.value));
                    });
                });
            slots::remember_resize(ui, table_id);
        },
    );
}

pub(super) fn changes(ui: &mut Ui, report: &ReportView<'_>, store: &StoreSnapshot) {
    touch_default(ui, store);
    defaults(ui, report, store);
    let mut changed = false;
    for account in &store.accounts {
        let initial = report.accounts.iter().find(|entry| {
            entry.address == account.address
                && report.input_namespace() != Some(entry.address.as_str())
        });
        let differs = initial != Some(account);
        if !differs {
            continue;
        }
        changed = true;
        egui::CollapsingHeader::new(&account.address)
            .id_salt(("changed_account", &account.address))
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(("account_changes", &account.address))
                    .num_columns(2)
                    .show(ui, |ui| {
                        value::text(ui, "Existence", format!("{:?}", account.existence));
                        if initial.is_none_or(|old| old.balance != account.balance) {
                            value::named(ui, "Balance after", &account.balance);
                        }
                        if initial.is_none_or(|old| old.nonce != account.nonce) {
                            value::named(ui, "Nonce after", &account.nonce);
                        }
                        if initial.is_none_or(|old| {
                            old.code_hash != account.code_hash || old.code_kind != account.code_kind
                        }) {
                            value::text(
                                ui,
                                "Code after",
                                format!(
                                    "{:?} · {}",
                                    account.code_kind,
                                    account.code_hash.as_deref().unwrap_or("unknown hash")
                                ),
                            );
                        }
                        value::text(ui, "Created", certainty(account.created));
                        value::text(ui, "Touched in transaction", touch_status(account.touched));
                        value::text(
                            ui,
                            "Pending destruction",
                            certainty(account.pending_destruction),
                        );
                    });
                for transient in [false, true] {
                    let root = if transient {
                        account.transient_root
                    } else {
                        account.storage_root
                    };
                    super::symbolic::show(
                        ui,
                        report,
                        root.node,
                        if transient {
                            "Transient state after"
                        } else {
                            "Persistent state after"
                        },
                    );
                    let changes = changed_slots(initial, account, transient);
                    let title = if transient {
                        "Transient slots after"
                    } else {
                        "Persistent slots after"
                    };
                    let default = if transient {
                        &account.transient_default
                    } else {
                        &account.storage_default
                    };
                    let default_changed = initial.is_none_or(|old| {
                        if transient {
                            old.transient_default != account.transient_default
                        } else {
                            old.storage_default != account.storage_default
                        }
                    });
                    if changes.is_empty() && !default_changed {
                        continue;
                    }
                    ui.label(RichText::new(title).strong());
                    if default_changed {
                        ui.horizontal(|ui| {
                            ui.label("Base slot default after");
                            value::cell(ui, default);
                        });
                    }
                    if !changes.is_empty() {
                        egui::ScrollArea::both()
                            .id_salt(("outcome_slots", &account.address, transient))
                            .max_height(160.0)
                            .auto_shrink([false, true])
                            .show_rows(ui, 21.0, changes.len(), |ui, range| {
                                for index in range {
                                    let (slot, after) = changes[index];
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Label::new(RichText::new(slot).monospace())
                                                .truncate(),
                                        )
                                        .on_hover_text(slot);
                                        value::cell(ui, after);
                                    });
                                }
                            });
                    }
                }
            });
    }
    if !changed {
        ui.label("No explicit account differences relative to the captured initial observations.");
    }
    ui.label(
        RichText::new(format!(
            "{} retained log sites{}",
            store.logs.len(),
            if store.logs_unknown {
                " · additional logs may exist"
            } else {
                ""
            }
        ))
        .color(palette::MUTED),
    );
    for log in &store.logs {
        egui::CollapsingHeader::new(format!(
            "{} · {:?} · pc 0x{:x} · {} topics",
            log.address,
            log.source_kind,
            log.pc,
            log.topics.len()
        ))
        .id_salt((
            "log_site",
            &log.address,
            &log.code_address,
            &log.code_hash,
            log.source_kind,
            log.pc,
        ))
        .show(ui, |ui| {
            ui.monospace(format!("Code account: {}", log.code_address));
            ui.monospace(format!("Code hash: {}", log.code_hash));
            for (index, topic) in log.topics.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Topic {index}"));
                    value::cell(ui, topic);
                });
            }
            ui.allocate_ui(egui::vec2(ui.available_width(), 100.0), |ui| {
                super::buffers::show(ui, report, log.data, "Log data")
            });
        });
    }
}

fn defaults(ui: &mut Ui, report: &ReportView<'_>, store: &StoreSnapshot) {
    ui.collapsing("Unlisted accounts and storage bases", |ui| {
        egui::Grid::new("store_defaults")
            .num_columns(2)
            .show(ui, |ui| {
                value::named(ui, "Persistent slot default", &store.storage_default);
                value::named(ui, "Transient slot default", &store.transient_default);
                value::named(ui, "Unlisted account balance", &store.balance_default);
                value::named(ui, "Unlisted account nonce", &store.nonce_default);
                value::text(
                    ui,
                    "Unlisted account existence",
                    format!("{:?}", store.existence_default),
                );
                match &store.code_default {
                    None | Some(AccountCode::Unknown) => {
                        value::text(ui, "Unlisted account code", "Unknown");
                    }
                    Some(AccountCode::Empty) => {
                        value::text(ui, "Unlisted account code", "Empty");
                    }
                    Some(AccountCode::Runtime(bytes)) => {
                        value::text(ui, "Unlisted account code", "Runtime");
                        value::text(ui, "Runtime bytes", bytes);
                    }
                    Some(AccountCode::Delegation(target)) => {
                        value::text(ui, "Unlisted account code", "Delegation");
                        value::text(ui, "Delegation implementation", target);
                    }
                }
                value::text(ui, "Unlisted account creation", certainty(store.created_default));
                value::text(
                    ui,
                    "Unlisted account pending destruction",
                    certainty(store.pending_destruction_default),
                );
                value::text(ui, "Unlisted account touch", touch_status(store.touched_default));
            });
        ui.label(
            "Storage bases are scoped by account owner when read. A shared base does not imply equal values for different owners.",
        );
        super::symbolic::show(ui, report, store.storage_root.node, "Persistent base template");
        super::symbolic::show(ui, report, store.transient_root.node, "Transient base template");
        super::symbolic::facts(ui, &store.storage_facts, "Persistent store facts");
        super::symbolic::facts(ui, &store.transient_facts, "Transient store facts");
    });
}

fn touch_default(ui: &mut Ui, store: &StoreSnapshot) {
    ui.horizontal(|ui| {
        ui.label("Unlisted account touch");
        ui.monospace(touch_status(store.touched_default));
    });
}

fn touch_status(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Must be touched",
        Some(false) => "Untouched",
        None => "May be touched",
    }
}

fn certainty(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "Unknown",
    }
}

// Compare semantic values over the union of explicit keys. Removing a cell can
// change it to the new default (for example after account destruction).
fn changed_slots<'a>(
    initial: Option<&'a AccountState>,
    account: &'a AccountState,
    transient: bool,
) -> Vec<(&'a str, &'a ValueInfo)> {
    let after = if transient {
        &account.transient
    } else {
        &account.storage
    };
    let default = if transient {
        &account.transient_default
    } else {
        &account.storage_default
    };
    let before = initial.map(|old| {
        if transient {
            &old.transient
        } else {
            &old.storage
        }
    });
    let before_default = initial.map(|old| {
        if transient {
            &old.transient_default
        } else {
            &old.storage_default
        }
    });
    let keys: std::collections::BTreeSet<_> = after
        .iter()
        .map(|entry| entry.slot.as_str())
        .chain(
            before
                .into_iter()
                .flatten()
                .map(|entry| entry.slot.as_str()),
        )
        .collect();
    keys.into_iter()
        .filter_map(|key| {
            let after_value = after
                .iter()
                .find(|entry| entry.slot == key)
                .map_or(default, |entry| &entry.value);
            let before_value = before
                .and_then(|entries| entries.iter().find(|entry| entry.slot == key))
                .map(|entry| &entry.value)
                .or(before_default);
            (before_value != Some(after_value)).then_some((key, after_value))
        })
        .collect()
}

#[cfg(test)]
pub(super) fn changed_slots_for_test<'a>(
    initial: Option<&'a AccountState>,
    account: &'a AccountState,
    transient: bool,
) -> Vec<(&'a str, &'a ValueInfo)> {
    changed_slots(initial, account, transient)
}
