//! Local decompiler draft. Editing never changes the visible immutable artifact.

use crate::{
    controls::{self, Density},
    framework, palette,
};
use evm_abstract_protocol::{
    DecompileLimits, DecompileRequest, DecompileStrategy, Fork,
    decompile_model::{ExecutionDomain, ReadabilityConfig},
};

pub(super) struct Form {
    pub open: bool,
    pub bytecode: String,
    fork: Fork,
    execution_domain: ExecutionDomain,
    strategy: DecompileStrategy,
    limits: DecompileLimits,
    readability: ReadabilityConfig,
    readability_scope: Option<Box<DecompileRequest>>,
    readability_notice: bool,
    limit_inputs: framework::NumberInputs,
    keyboard: framework::SubmitKeyboard,
    replace_editor: bool,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            open: true,
            bytecode: "5f35600a576001600d565b60025b60030100".into(),
            fork: Fork::Osaka,
            execution_domain: ExecutionDomain::Generic,
            strategy: DecompileStrategy::Dispatcher,
            limits: DecompileLimits::default(),
            readability: ReadabilityConfig::default(),
            readability_scope: None,
            readability_notice: false,
            limit_inputs: framework::NumberInputs::default(),
            keyboard: framework::SubmitKeyboard::default(),
            replace_editor: false,
        }
    }
}

impl Form {
    pub fn remember_readability(&mut self, request: &DecompileRequest) {
        self.readability = request.readability.clone();
        self.readability_scope = Some(Box::new(request.clone()));
        self.readability_notice = false;
    }

    fn clear_stale_regions(&mut self) {
        if self.readability.regions.is_empty() {
            return;
        }
        let compact: String = self
            .bytecode
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let code = compact
            .strip_prefix("0x")
            .or_else(|| compact.strip_prefix("0X"))
            .unwrap_or(&compact);
        let current = self.readability_scope.as_ref().is_some_and(|scope| {
            let previous = scope.bytecode.strip_prefix("0x").unwrap_or(&scope.bytecode);
            code.eq_ignore_ascii_case(previous)
                && self.fork == scope.fork
                && self.execution_domain == scope.execution_domain
                && self.strategy == scope.strategy
                && self.limits == scope.limits
        });
        if !current {
            self.readability.regions.clear();
            self.readability_notice = true;
        }
    }

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

    fn can_submit(&self, busy: bool) -> bool {
        !busy && self.limit_inputs.valid() && self.validation().is_none()
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
        let input_response = ui.push_id("decompile_input", |ui| {
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
                    ui.label("Execution scope");
                    let domain = egui::ComboBox::from_id_salt("decompile_execution_domain")
                        .selected_text(super::domain::label(self.execution_domain))
                        .show_ui(ui, |ui| {
                            for domain in [
                                ExecutionDomain::Generic,
                                ExecutionDomain::EthereumTransactions,
                            ] {
                                controls::tab_value(
                                    ui,
                                    &mut self.execution_domain,
                                    domain,
                                    super::domain::label(domain),
                                    Density::Regular,
                                );
                            }
                        });
                    controls::observe(ui, &domain.response, "Execution scope", "combobox", None);
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
            let assumption = super::domain::assumption(self.execution_domain);
            let response = ui.label(
                egui::RichText::new(assumption)
                    .small()
                    .color(palette::MUTED),
            );
            controls::observe(
                ui,
                &response,
                assumption,
                "decompile-domain-assumption",
                None,
            );
            super::readability::global_controls(ui, &mut self.readability);
            framework::section(ui, "Decompile budgets", |ui| {
                egui::Grid::new("decompile_limits")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Work limit", &mut self.limits.max_work),
                            ("Block limit", &mut self.limits.max_blocks),
                            ("Operation limit", &mut self.limits.max_operations),
                            ("Clone limit", &mut self.limits.max_cloned_blocks),
                            ("History limit", &mut self.limits.max_history_entities),
                            ("Recovery iteration limit", &mut self.limits.max_iterations),
                        ] {
                            self.limit_inputs.number(ui, label, value);
                        }
                    });
            });
            self.clear_stale_regions();
            if self.readability_notice {
                ui.colored_label(
                    palette::WARNING,
                    "Region choices cleared after the input or analysis settings changed.",
                );
            }
            if let Some(error) = self.validation() {
                ui.colored_label(palette::WARNING, error);
            }
            ui.horizontal_wrapped(|ui| {
                submit = controls::action_enabled(
                    ui,
                    self.can_submit(busy),
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
                    } else if !self.limit_inputs.valid() {
                        "Correct invalid budgets: use whole numbers from 0 to 18446744073709551615"
                    } else if self.validation().is_some() {
                        "Check the highlighted input"
                    } else {
                        "Ctrl+Enter to submit"
                    })
                    .small()
                    .color(palette::MUTED),
                );
            });
        });
        controls::observe(
            ui,
            &input_response.response,
            "Decompiler input",
            "decompile-input",
            None,
        );
        if (submit || keyboard_submit) && self.can_submit(busy) {
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
                execution_domain: self.execution_domain,
                strategy: self.strategy,
                limits: self.limits.clone(),
                readability: self.readability.clone(),
            })
        } else {
            None
        }
    }

    pub fn status(&self) -> String {
        if self.open {
            format!(
                "Decompiler input: {:?}; Execution scope draft: {}; Strategy draft: {:?}; Readability draft: {:?}; Procedures draft: {:?}; Input validation: {}",
                self.fork,
                super::domain::label(self.execution_domain),
                self.strategy,
                self.readability.profile,
                self.readability.procedures,
                if !self.limit_inputs.valid() {
                    "Correct invalid decompile budgets"
                } else {
                    self.validation().unwrap_or("Ready")
                }
            )
        } else {
            String::new()
        }
    }
}
