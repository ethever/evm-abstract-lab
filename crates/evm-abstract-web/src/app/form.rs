//! Modal request editor. UI strings are confined to lossless protocol scalar
//! fields; submission always produces the shared, typed analysis request.

mod environment;
mod examples;
mod limits;
mod shortcut;
mod validation;

use crate::controls::{self, Density};
use egui::{Id, RichText, Ui};
use evm_abstract_protocol::{
    AnalysisInput, AnalysisLimits, AnalyzeRequest, BlockSelector, BytecodeInput, EnvironmentInput,
    Fork, RpcInput, RpcProvidersReply,
};

use super::{BRANCH_EXAMPLE, Command, TransportError, providers::RpcProviders};
use crate::{framework, notation, palette};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Bytecode,
    Rpc,
}

/// Editing is independent of a submitted job and its immutable result.
pub(super) struct AnalysisForm {
    pub(super) open: bool,
    was_open: bool,
    source: Source,
    pub(super) bytecode: BytecodeInput,
    rpc: RpcInput,
    providers: RpcProviders,
    fork: Fork,
    environment: EnvironmentInput,
    limits: AnalysisLimits,
    limit_inputs: limits::NumberInputs,
    block_inputs: limits::NumberInputs,
    keyboard: shortcut::Keyboard,
    pub(super) error: Option<String>,
}

impl Default for AnalysisForm {
    fn default() -> Self {
        let defaults = AnalyzeRequest::default();
        Self {
            open: false,
            was_open: false,
            source: Source::Bytecode,
            bytecode: BytecodeInput {
                bytecode: BRANCH_EXAMPLE.into(),
                address: None,
            },
            rpc: RpcInput {
                provider_id: String::new(),
                address: examples::Example::Usdc.address().into(),
                block: BlockSelector::Latest,
                accounts: vec![],
            },
            providers: RpcProviders::default(),
            fork: defaults.fork,
            environment: defaults.environment,
            limits: defaults.limits,
            limit_inputs: limits::NumberInputs::default(),
            block_inputs: limits::NumberInputs::default(),
            keyboard: shortcut::Keyboard::default(),
            error: None,
        }
    }
}

impl AnalysisForm {
    /// Select the prepared RPC entry only when its configured provider is usable.
    /// Startup owns when to call this; rendering never resets the address draft.
    pub(super) fn select_rpc_example_if_available(&mut self) -> bool {
        if self.providers.valid_selection(&self.rpc.provider_id) {
            self.source = Source::Rpc;
            true
        } else {
            false
        }
    }

    pub(super) fn provider_command(&mut self) -> Option<Command> {
        self.providers.next_command()
    }

    pub(super) fn provider_loading_generation(&self) -> Option<u64> {
        self.providers.loading_generation()
    }

    pub(super) fn receive_providers(
        &mut self,
        generation: u64,
        result: Result<RpcProvidersReply, TransportError>,
    ) -> bool {
        self.providers
            .receive(generation, result, &mut self.rpc.provider_id)
    }

    pub(super) fn provider_status(&self) -> String {
        self.providers.status(&self.rpc.provider_id)
    }

    fn can_submit(&self, busy: bool) -> bool {
        !busy
            && self.limit_inputs.valid()
            && self.validation_error().is_none()
            && (self.source == Source::Bytecode
                || (self.block_inputs.valid()
                    && self.providers.valid_selection(&self.rpc.provider_id)))
    }

    pub(super) fn request(&self) -> AnalyzeRequest {
        AnalyzeRequest {
            input: match self.source {
                Source::Bytecode => AnalysisInput::Bytecode(self.bytecode.clone()),
                Source::Rpc => AnalysisInput::Rpc(self.rpc.clone()),
            },
            fork: self.fork,
            environment: self.environment.clone(),
            limits: self.limits.clone(),
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context, busy: bool) -> Option<AnalyzeRequest> {
        if !self.open {
            self.was_open = false;
            self.keyboard.reset();
            return None;
        }
        let keyboard_submit = self.keyboard.submit(ctx);
        let size = ctx.content_rect().size();
        let action_density = if size.x < 680.0 {
            Density::Touch
        } else {
            Density::Regular
        };
        let mut submit = false;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("analysis_form"))
            .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(10))
            .show(ctx, |ui| {
                ui.set_width((size.x - 40.0).clamp(180.0, 740.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("New analysis").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = controls::action(ui, "Close", action_density).clicked();
                    });
                });
                let budget = (size.y - 128.0).clamp(40.0, 620.0);
                egui::ScrollArea::vertical()
                    .id_salt("analysis_form_body")
                    .max_height(budget)
                    .min_scrolled_height(budget)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        self.input(ui);
                        self.environment(ui);
                        self.limits(ui);
                        if let Some(error) = self.validation_error() {
                            ui.colored_label(palette::WARNING, error.to_string());
                        }
                        if let Some(error) = &self.error {
                            ui.label(notation::widget(
                                ui,
                                RichText::new(error).color(palette::ERROR),
                            ));
                        }
                    });
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    submit = controls::action_enabled(
                        ui,
                        self.can_submit(busy),
                        RichText::new("Analyze").strong(),
                        action_density,
                    )
                    .on_hover_text("Ctrl+Enter")
                    .clicked();
                    ui.label(
                        RichText::new(if busy {
                            "A task is still running"
                        } else if !self.limit_inputs.valid()
                            || (self.source == Source::Rpc && !self.block_inputs.valid())
                        {
                            "Correct invalid numeric settings to analyze"
                        } else if self.validation_error().is_some() {
                            "Check the highlighted input"
                        } else if !self.can_submit(false) {
                            "Choose an available RPC provider to analyze"
                        } else {
                            "Ctrl+Enter to submit"
                        })
                        .small()
                        .color(palette::MUTED),
                    );
                });
            });
        if !self.was_open {
            let id = match self.source {
                Source::Bytecode => "runtime_bytecode",
                Source::Rpc => "rpc_address",
            };
            ctx.memory_mut(|memory| memory.request_focus(Id::new(id)));
            self.was_open = true;
        }
        if self.can_submit(busy) && keyboard_submit {
            submit = true;
        }
        if close || modal.should_close() {
            self.open = false;
            self.was_open = false;
            self.keyboard.reset();
        }
        if submit {
            self.error = None;
            self.open = false;
            self.was_open = false;
            self.keyboard.reset();
            Some(self.request())
        } else {
            None
        }
    }

    fn input(&mut self, ui: &mut Ui) {
        let previous_source = self.source;
        let source_group = ui.horizontal_wrapped(|ui| {
            controls::tab_value(
                ui,
                &mut self.source,
                Source::Rpc,
                "Address",
                Density::Regular,
            );
            controls::tab_value(
                ui,
                &mut self.source,
                Source::Bytecode,
                "Bytecode",
                Density::Regular,
            );
            let observed_form_fork = egui::ComboBox::from_id_salt("form_fork")
                .width(82.0)
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
            controls::observe(
                ui,
                &observed_form_fork.response,
                "form_fork",
                "combobox",
                None,
            );
        });
        controls::observe(ui, &source_group.response, "input_source", "group", None);
        if previous_source != self.source {
            let id = match self.source {
                Source::Bytecode => "runtime_bytecode",
                Source::Rpc => "rpc_address",
            };
            ui.memory_mut(|memory| memory.request_focus(Id::new(id)));
        }
        ui.label(
            RichText::new(match self.source {
                Source::Rpc => "Analyze an account at one pinned RPC snapshot.",
                Source::Bytecode => "Analyze pasted runtime bytecode without RPC acquisition.",
            })
            .small()
            .color(palette::MUTED),
        );
        match self.source {
            Source::Bytecode => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Runtime bytecode").color(palette::MUTED));
                    controls::menu_button(ui, "Examples", Density::Regular, |ui| {
                        for (label, code) in [
                            ("Branch and merge", BRANCH_EXAMPLE),
                            ("Arithmetic", "600160020100"),
                        ] {
                            if controls::action(ui, label, Density::Regular).clicked() {
                                self.bytecode.bytecode = code.into();
                                ui.close();
                            }
                        }
                    });
                });
                framework::bytecode_editor(ui, &mut self.bytecode.bytecode, 80.0);
                optional_text(
                    ui,
                    "Root address",
                    "bytecode_address",
                    &mut self.bytecode.address,
                    "Symbolic ADDRESS when omitted",
                    "0x…",
                );
            }
            Source::Rpc => {
                self.providers.show(ui, &mut self.rpc.provider_id);
                examples::root_account(ui, &mut self.rpc.address);
                let old_kind = std::mem::discriminant(&self.rpc.block);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Snapshot block");
                    let choice = match self.rpc.block {
                        BlockSelector::Latest => "Latest",
                        BlockSelector::Number(_) => "Number",
                        BlockSelector::Hash(_) => "Hash",
                    };
                    let observed_rpc_block_kind = egui::ComboBox::from_id_salt("rpc_block_kind")
                        .width(80.0)
                        .selected_text(choice)
                        .show_ui(ui, |ui| {
                            if controls::tab(
                                ui,
                                matches!(self.rpc.block, BlockSelector::Latest),
                                "Latest",
                                Density::Regular,
                            )
                            .clicked()
                                && !matches!(self.rpc.block, BlockSelector::Latest)
                            {
                                self.rpc.block = BlockSelector::Latest;
                            }
                            if controls::tab(
                                ui,
                                matches!(self.rpc.block, BlockSelector::Number(_)),
                                "Number",
                                Density::Regular,
                            )
                            .clicked()
                                && !matches!(self.rpc.block, BlockSelector::Number(_))
                            {
                                self.rpc.block = BlockSelector::Number(0);
                            }
                            if controls::tab(
                                ui,
                                matches!(self.rpc.block, BlockSelector::Hash(_)),
                                "Hash",
                                Density::Regular,
                            )
                            .clicked()
                                && !matches!(self.rpc.block, BlockSelector::Hash(_))
                            {
                                self.rpc.block = BlockSelector::Hash(String::new());
                            }
                        });
                    controls::observe(
                        ui,
                        &observed_rpc_block_kind.response,
                        "rpc_block_kind",
                        "combobox",
                        None,
                    );
                    match &mut self.rpc.block {
                        BlockSelector::Latest => {
                            ui.label(
                                RichText::new("Pinned once when the task starts")
                                    .small()
                                    .color(palette::MUTED),
                            );
                        }
                        BlockSelector::Number(_) => {}
                        BlockSelector::Hash(hash) => {
                            framework::line_editor(ui, "rpc_block_hash", hash, "0x… (32 bytes)");
                        }
                    }
                });
                if old_kind != std::mem::discriminant(&self.rpc.block) {
                    self.block_inputs = limits::NumberInputs::default();
                }
                if let BlockSelector::Number(number) = &mut self.rpc.block {
                    egui::Grid::new("snapshot_number")
                        .num_columns(2)
                        .show(ui, |ui| {
                            self.block_inputs.number(ui, "Block number", number);
                        });
                }
                self.seed_accounts(ui);
            }
        }
    }

    fn seed_accounts(&mut self, ui: &mut Ui) {
        framework::section(ui, "Initial accounts and storage slots", |ui| {
            let mut remove = None;
            for (index, account) in self.rpc.accounts.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 72.0).max(60.0);
                        framework::scoped_editor(
                            ui,
                            "account",
                            &mut account.address,
                            "Account address",
                            width,
                        );
                        if controls::action(ui, "Remove", Density::Compact).clicked() {
                            remove = Some(index);
                        }
                    });
                    let mut remove_slot = None;
                    for (slot_index, slot) in account.slots.iter_mut().enumerate() {
                        ui.push_id(slot_index, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Slot");
                                let width = (ui.available_width() - 36.0).max(60.0);
                                framework::scoped_editor(ui, "slot", slot, "0x…", width);
                                if controls::action(ui, "−", Density::Compact).clicked() {
                                    remove_slot = Some(slot_index);
                                }
                            });
                        });
                    }
                    if let Some(index) = remove_slot {
                        account.slots.remove(index);
                    }
                    if controls::action(ui, "Add slot", Density::Compact).clicked() {
                        account.slots.push("0x0".into());
                    }
                });
                ui.separator();
            }
            if let Some(index) = remove {
                self.rpc.accounts.remove(index);
            }
            if controls::action(ui, "Add account", Density::Compact).clicked() {
                self.rpc.accounts.push(evm_abstract_protocol::AccountQuery {
                    address: String::new(),
                    slots: vec![],
                });
            }
        });
    }
}

impl super::Workspace {
    /// Input mode and draft validity for the open canvas editor's live region.
    pub fn accessible_input_status(&self) -> Option<String> {
        if !self.form.open {
            return None;
        }
        let mode = match self.form.source {
            Source::Bytecode => "Bytecode",
            Source::Rpc => "Address",
        };
        let validation = if let Some(error) = self.form.validation_error() {
            error.to_string()
        } else if !self.form.limit_inputs.valid()
            || (self.form.source == Source::Rpc && !self.form.block_inputs.valid())
        {
            "Correct invalid numeric settings".into()
        } else if self.form.source == Source::Rpc
            && !self
                .form
                .providers
                .valid_selection(&self.form.rpc.provider_id)
        {
            "Choose an available RPC provider".into()
        } else {
            "Ready".into()
        };
        Some(format!(
            "Input editor: {mode}; Input validation: {validation}"
        ))
    }
}

fn optional_text(
    ui: &mut Ui,
    label: &str,
    id: &str,
    value: &mut Option<String>,
    default: &str,
    hint: &str,
) {
    let mut enabled = value.is_some();
    if framework::checkbox(ui, &mut enabled, label).changed() {
        *value = enabled.then(String::new);
    }
    if let Some(value) = value {
        framework::line_editor(ui, id, value, hint);
    } else {
        ui.label(RichText::new(default).small().color(palette::MUTED));
    }
}

#[cfg(test)]
mod tests;
