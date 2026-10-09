//! Persistent/transient storage and finalized state changes remain separate.

use super::value;
use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AccountState, AnalysisReport, StoreSnapshot, ValueInfo};

pub(super) fn account<'a>(
    store: Option<&'a StoreSnapshot>,
    report: &'a AnalysisReport,
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
    report: &AnalysisReport,
    store: Option<&StoreSnapshot>,
    address: &str,
    transient: bool,
) {
    ui.label(RichText::new(address).monospace());
    let account = account(store, report, address);
    if let Some(account) = account {
        let facts_height = (ui.available_height() * 0.45).clamp(0.0, 80.0);
        ui.collapsing("Account facts in this store", |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("storage_account_facts_scroll", address, transient))
                .max_height(facts_height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new(("storage_account_facts", address, transient))
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
                                value::text(ui, "Captured program", format!("P{program}"));
                            }
                            value::text(ui, "Created in transaction", certainty(account.created));
                            value::text(
                                ui,
                                "Pending destruction",
                                certainty(account.pending_destruction),
                            );
                        });
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
            ui.label("Unspecified slots");
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
        ui.label("No explicit slots in this snapshot; unspecified slots use the default above.");
        return;
    };
    egui::ScrollArea::both()
        .id_salt(("storage_slots", transient, address))
        .auto_shrink([false, false])
        .show_rows(ui, 21.0, entries.len(), |ui, range| {
            for index in range {
                let entry = &entries[index];
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&entry.slot).monospace()).truncate())
                        .on_hover_text(&entry.slot);
                    value::cell(ui, &entry.value);
                });
            }
        });
}

pub(super) fn changes(ui: &mut Ui, report: &AnalysisReport, store: &StoreSnapshot) {
    let mut changed = false;
    for account in &store.accounts {
        let initial = report
            .accounts
            .iter()
            .find(|entry| entry.address == account.address);
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
                        value::text(
                            ui,
                            "Pending destruction",
                            certainty(account.pending_destruction),
                        );
                    });
                for transient in [false, true] {
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
                            ui.label("Unspecified slots after");
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
        ui.collapsing(
            format!(
                "{} · pc 0x{:x} · {} topics",
                log.address,
                log.pc,
                log.topics.len()
            ),
            |ui| {
                ui.monospace(format!("Code account: {}", log.code_address));
                for (index, topic) in log.topics.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Topic {index}"));
                        value::cell(ui, topic);
                    });
                }
                ui.allocate_ui(egui::vec2(ui.available_width(), 100.0), |ui| {
                    super::buffers::show(ui, report, log.data, "Log data")
                });
            },
        );
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
