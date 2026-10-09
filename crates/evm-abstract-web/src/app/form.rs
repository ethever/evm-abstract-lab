//! Modal request editor. UI strings are confined to lossless protocol scalar
//! fields; submission always produces the shared, typed analysis request.

mod environment;
mod examples;
mod limits;

use egui::{Id, RichText, Ui};
use evm_abstract_protocol::{
    AnalysisInput, AnalysisLimits, AnalyzeRequest, BlockSelector, BytecodeInput, EnvironmentInput,
    Fork, RpcInput, RpcProvidersReply,
};

use super::{BRANCH_EXAMPLE, Command, TransportError, providers::RpcProviders};
use crate::{framework, palette};

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
            && (self.source == Source::Bytecode
                || self.providers.valid_selection(&self.rpc.provider_id))
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
            return None;
        }
        let size = ctx.content_rect().size();
        let mut submit = false;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("analysis_form"))
            .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(10))
            .show(ctx, |ui| {
                ui.set_width((size.x - 40.0).clamp(180.0, 740.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("New analysis").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui.small_button("Close").clicked();
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
                        if let Some(error) = &self.error {
                            ui.colored_label(palette::ERROR, error);
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    submit = ui
                        .add_enabled(
                            self.can_submit(busy),
                            egui::Button::new(
                                RichText::new("Analyze").strong().color(palette::BACKGROUND),
                            )
                            .fill(palette::ACCENT),
                        )
                        .on_hover_text("Ctrl+Enter")
                        .clicked();
                    ui.label(
                        RichText::new(if busy {
                            "A task is still running"
                        } else if !self.limit_inputs.valid() {
                            "Correct invalid numeric settings to analyze"
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
        if self.can_submit(busy)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter))
        {
            submit = true;
        }
        if close || modal.should_close() {
            self.open = false;
        }
        if submit {
            self.error = None;
            self.open = false;
            Some(self.request())
        } else {
            None
        }
    }

    fn input(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.source, Source::Bytecode, "Bytecode");
            ui.selectable_value(&mut self.source, Source::Rpc, "RPC account");
            egui::ComboBox::from_id_salt("form_fork")
                .width(82.0)
                .selected_text(format!("{:?}", self.fork))
                .show_ui(ui, |ui| {
                    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
                        ui.selectable_value(&mut self.fork, fork, format!("{fork:?}"));
                    }
                });
        });
        match self.source {
            Source::Bytecode => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Runtime bytecode").color(palette::MUTED));
                    ui.menu_button("Examples", |ui| {
                        for (label, code) in [
                            ("Branch and merge", BRANCH_EXAMPLE),
                            ("Arithmetic", "600160020100"),
                        ] {
                            if ui.button(label).clicked() {
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
                ui.horizontal_wrapped(|ui| {
                    ui.label("Snapshot block");
                    let choice = match self.rpc.block {
                        BlockSelector::Latest => "Latest",
                        BlockSelector::Number(_) => "Number",
                        BlockSelector::Hash(_) => "Hash",
                    };
                    egui::ComboBox::from_id_salt("rpc_block_kind")
                        .width(80.0)
                        .selected_text(choice)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    matches!(self.rpc.block, BlockSelector::Latest),
                                    "Latest",
                                )
                                .clicked()
                            {
                                self.rpc.block = BlockSelector::Latest;
                            }
                            if ui
                                .selectable_label(
                                    matches!(self.rpc.block, BlockSelector::Number(_)),
                                    "Number",
                                )
                                .clicked()
                            {
                                self.rpc.block = BlockSelector::Number(0);
                            }
                            if ui
                                .selectable_label(
                                    matches!(self.rpc.block, BlockSelector::Hash(_)),
                                    "Hash",
                                )
                                .clicked()
                            {
                                self.rpc.block = BlockSelector::Hash(String::new());
                            }
                        });
                    match &mut self.rpc.block {
                        BlockSelector::Latest => {
                            ui.label(
                                RichText::new("Pinned once when the task starts")
                                    .small()
                                    .color(palette::MUTED),
                            );
                        }
                        BlockSelector::Number(number) => {
                            ui.add(egui::DragValue::new(number));
                        }
                        BlockSelector::Hash(hash) => {
                            framework::line_editor(ui, "rpc_block_hash", hash, "0x… (32 bytes)");
                        }
                    }
                });
                self.seed_accounts(ui);
            }
        }
    }

    fn seed_accounts(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Initial accounts and storage slots").show(ui, |ui| {
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
                        if ui.small_button("Remove").clicked() {
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
                                if ui.small_button("−").clicked() {
                                    remove_slot = Some(slot_index);
                                }
                            });
                        });
                    }
                    if let Some(index) = remove_slot {
                        account.slots.remove(index);
                    }
                    if ui.small_button("Add slot").clicked() {
                        account.slots.push("0x0".into());
                    }
                });
                ui.separator();
            }
            if let Some(index) = remove {
                self.rpc.accounts.remove(index);
            }
            if ui.small_button("Add account").clicked() {
                self.rpc.accounts.push(evm_abstract_protocol::AccountQuery {
                    address: String::new(),
                    slots: vec![],
                });
            }
        });
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
    if ui.checkbox(&mut enabled, label).changed() {
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
