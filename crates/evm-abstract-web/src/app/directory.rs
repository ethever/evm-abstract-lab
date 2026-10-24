//! Captured programs and observed accounts share native identities with selection.

use crate::controls::{self, Density};
use crate::{framework, notation, palette};
use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::AnalysisReport;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Programs,
    Accounts,
}

#[derive(Default)]
pub(super) struct Directory {
    pub(super) open: bool,
    kind: Kind,
    filter: String,
}

pub(super) enum Pick {
    Program(usize),
    Account(String),
    RootInput,
}

impl Directory {
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        selected_program: Option<usize>,
        selected_account: Option<&str>,
    ) -> Option<Pick> {
        Density::Compact.scope(ui, |ui| {
            ui.horizontal(|ui| {
                controls::tab_value(
                    ui,
                    &mut self.kind,
                    Kind::Programs,
                    "Programs",
                    Density::Compact,
                );
                controls::tab_value(
                    ui,
                    &mut self.kind,
                    Kind::Accounts,
                    "Accounts",
                    Density::Compact,
                );
            });
        });
        framework::line_editor(
            ui,
            "directory_filter",
            &mut self.filter,
            "Filter address / hash",
        );
        let filter = self.filter.trim().to_ascii_lowercase();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("world_directory")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.kind {
                Kind::Programs => {
                    for program in &report.programs {
                        if !filter.is_empty()
                            && !program
                                .code_address
                                .as_deref()
                                .unwrap_or("")
                                .to_ascii_lowercase()
                                .contains(&filter)
                            && !program.code_hash.to_ascii_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        let title = format!(
                            "{} · {}",
                            Symbol::Program(program.id),
                            short(program.code_address.as_deref().unwrap_or(
                                if program.kind == evm_abstract_protocol::CodeKind::Runtime {
                                    "Supplied input program"
                                } else {
                                    "Constructor source"
                                }
                            ))
                        );
                        if ui
                            .add(
                                egui::Button::selectable(
                                    selected_program == Some(program.id),
                                    notation::widget(ui, RichText::new(title).monospace()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(format!(
                                "{:?}\n{}\n{}",
                                program.kind,
                                program.code_address.as_deref().unwrap_or(
                                    if program.kind == evm_abstract_protocol::CodeKind::Runtime {
                                        "Supplied input program"
                                    } else {
                                        "Constructor source"
                                    }
                                ),
                                program.code_hash
                            ))
                            .clicked()
                        {
                            picked = Some(Pick::Program(program.id));
                        }
                        ui.label(
                            RichText::new(format!(
                                "{:?} · {} bytes · {} blocks",
                                program.kind,
                                program.bytecode.trim_start_matches("0x").len() / 2,
                                program.blocks.len()
                            ))
                            .small()
                            .color(palette::MUTED),
                        );
                    }
                    if report.programs.is_empty() {
                        ui.label("No executable bytecode captured.");
                    }
                }
                Kind::Accounts => {
                    ui.label(
                        RichText::new("Initial observations")
                            .small()
                            .color(palette::MUTED),
                    );
                    for account in &report.accounts {
                        if report.input_namespace() == Some(account.address.as_str()) {
                            if controls::action(ui, "Supplied root state", Density::Compact)
                                .on_hover_text("Input program/state template for symbolic ADDRESS")
                                .clicked()
                            {
                                picked = Some(Pick::RootInput);
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{:?} · {} supplied slots",
                                    account.code_kind,
                                    account.storage.len()
                                ))
                                .small()
                                .color(palette::MUTED),
                            );
                            continue;
                        }
                        if !filter.is_empty()
                            && !account.address.to_ascii_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        if ui
                            .selectable_label(
                                selected_account == Some(account.address.as_str()),
                                RichText::new(short(&account.address)).monospace(),
                            )
                            .on_hover_text(&account.address)
                            .clicked()
                        {
                            picked = Some(Pick::Account(account.address.clone()));
                        }
                        ui.label(
                            RichText::new(format!(
                                "{:?} · {:?} · {} slots",
                                account.existence,
                                account.code_kind,
                                account.storage.len()
                            ))
                            .small()
                            .color(palette::MUTED),
                        );
                    }
                }
            });
        picked
    }
}

pub(super) fn short(value: &str) -> String {
    if value.len() > 22 && value.is_ascii() {
        format!("{}…{}", &value[..10], &value[value.len() - 8..])
    } else {
        value.into()
    }
}
