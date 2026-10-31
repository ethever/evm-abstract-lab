//! Local decompiler draft. Editing never changes the visible immutable artifact.

use crate::{
    controls::{self, Density},
    framework, palette,
};
use evm_abstract_protocol::{DecompileLimits, DecompileRequest, DecompileStrategy, Fork};

pub(super) struct Form {
    pub open: bool,
    pub bytecode: String,
    fork: Fork,
    strategy: DecompileStrategy,
    limits: DecompileLimits,
    keyboard: framework::SubmitKeyboard,
    replace_editor: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            open: true,
            bytecode: "5f35600a576001600d565b60025b60030100".into(),
            fork: Fork::Osaka,
            strategy: DecompileStrategy::Dispatcher,
            limits: DecompileLimits::default(),
            keyboard: framework::SubmitKeyboard::default(),
            replace_editor: false,
        }
    }
}

impl Form {
    pub fn load(&mut self, bytecode: String, fork: Fork) {
        self.bytecode = bytecode;
        self.fork = fork;
        self.open = true;
        self.keyboard.reset();
        self.replace_editor = true;
    }

    pub fn validation(&self) -> Option<&'static str> {
        let compact: String = self
            .bytecode
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let digits = compact
            .strip_prefix("0x")
            .or_else(|| compact.strip_prefix("0X"))
            .unwrap_or(&compact);
        if !digits.len().is_multiple_of(2) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Some(
                "Runtime bytecode: enter pairs of hexadecimal digits; whitespace and 0x are accepted",
            )
        } else {
            None
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<DecompileRequest> {
        if !self.open {
            self.keyboard.reset();
            return None;
        }
        if std::mem::take(&mut self.replace_editor) {
            framework::BytecodeEditor::Decompiler.replace(ui.ctx());
            ui.memory_mut(|memory| {
                memory.request_focus(framework::BytecodeEditor::Decompiler.id())
            });
        }
        let keyboard_submit = self.keyboard.submit(ui.ctx());
        let mut submit = false;
        ui.push_id("decompile_input", |ui| {
            ui.label("Runtime bytecode");
            framework::bytecode_editor(
                ui,
                framework::BytecodeEditor::Decompiler,
                &mut self.bytecode,
                64.0,
            );
            Density::Regular.scope(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("EVM fork");
                    let fork = egui::ComboBox::from_id_salt("decompile_fork")
                        .selected_text(format!("{:?}", self.fork))
                        .show_ui(ui, |ui| {
                            for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
                                controls::tab_value(
                                    ui,
                                    &mut self.fork,
                                    fork,
                                    format!("{fork:?}"),
                                    Density::Regular,
                                );
                            }
                        });
                    controls::observe(ui, &fork.response, "EVM fork", "combobox", None);
                    ui.separator();
                    ui.label("Irreducible control flow");
                    controls::tab_value(
                        ui,
                        &mut self.strategy,
                        DecompileStrategy::Dispatcher,
                        "Dispatcher",
                        Density::Regular,
                    );
                    controls::tab_value(
                        ui,
                        &mut self.strategy,
                        DecompileStrategy::CodeDuplication,
                        "Code duplication",
                        Density::Regular,
                    );
                });
            });
            framework::section(ui, "Decompile budgets", |ui| {
                ui.horizontal_wrapped(|ui| {
                    budget(ui, "Work limit", &mut self.limits.max_work);
                    budget(ui, "Block limit", &mut self.limits.max_blocks);
                    budget(ui, "Operation limit", &mut self.limits.max_operations);
                    budget(ui, "Clone limit", &mut self.limits.max_cloned_blocks);
                    budget(ui, "History limit", &mut self.limits.max_history_entities);
                    budget(
                        ui,
                        "Recovery iteration limit",
                        &mut self.limits.max_iterations,
                    );
                });
            });
            if let Some(error) = self.validation() {
                ui.colored_label(palette::WARNING, error);
            }
            ui.horizontal_wrapped(|ui| {
                submit = controls::action_enabled(
                    ui,
                    !busy && self.validation().is_none(),
                    "Decompile",
                    Density::Regular,
                )
                .on_hover_text("Ctrl+Enter to decompile")
                .clicked();
                if controls::action(ui, "Hide input", Density::Compact).clicked() {
                    self.open = false;
                }
                ui.label(
                    egui::RichText::new(if busy {
                        "A decompiler task is active"
                    } else {
                        "Ctrl+Enter to submit"
                    })
                    .small()
                    .color(palette::MUTED),
                );
            });
        });
        if (submit || keyboard_submit) && !busy && self.validation().is_none() {
            let compact: String = self
                .bytecode
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            // Draft spelling belongs to the editor/undo history. Only the wire
            // payload canonicalizes the prefix accepted by the shared parser.
            let bytecode = if let Some(digits) = compact.strip_prefix("0X") {
                format!("0x{digits}")
            } else {
                compact
            };
            Some(DecompileRequest {
                bytecode,
                fork: self.fork,
                strategy: self.strategy,
                limits: self.limits.clone(),
            })
        } else {
            None
        }
    }

    pub fn status(&self) -> String {
        if self.open {
            format!(
                "Decompiler input: {:?}; Strategy draft: {:?}; Input validation: {}",
                self.fork,
                self.strategy,
                self.validation().unwrap_or("Ready")
            )
        } else {
            String::new()
        }
    }
}

fn budget(ui: &mut egui::Ui, label: &str, value: &mut usize) {
    ui.label(label);
    let response = ui.add(egui::DragValue::new(value).range(0..=usize::MAX).speed(1.0));
    controls::observe(ui, &response, label, "number", None);
}
