//! The explorer borrows the canonical index and emits navigation proposals.
//! Source grouping never combines the underlying context or state identities.

use egui::{RichText, Ui};
use evm_abstract_protocol::{BlockCoverage, ProcessOwner, SummarySourceStatus};

use super::{Directory, Kind, short};
use crate::{
    app::model::{
        ReportViewId,
        navigation::{EvidenceFocus, Focus, Intent},
        presentation::{Capability, Presentation, PublicationIdentity, ResolvedContext},
    },
    controls::{self, Density},
    framework, palette,
};

pub(crate) enum Action {
    Navigate(Intent),
    Account(String),
    MinimumAccount(usize),
    RootInput,
}

#[derive(Default)]
pub(super) struct Rows {
    key: Option<(PublicationIdentity, ReportViewId, Option<usize>)>,
    sources: Vec<Source>,
    contexts: Vec<Context>,
    entries: Vec<Entry>,
    filter: Option<String>,
}

struct Source {
    program: usize,
    title: String,
    detail: String,
    search: String,
    group: usize,
    group_size: usize,
}

struct Context {
    focus: Focus,
    title: String,
    detail: String,
    search: String,
    current: bool,
}

#[derive(Clone, Copy)]
enum Entry {
    Group(usize),
    Source(usize),
    Contexts,
    Context(usize),
}

impl Directory {
    pub(crate) fn show_scoped(
        &mut self,
        ui: &mut Ui,
        presentation: Presentation<'_>,
        focus: &Focus,
        selected_account: Option<&str>,
    ) -> Option<Action> {
        if let Err(error) = presentation.validate(focus) {
            ui.label(error.to_string());
            return None;
        }
        self.scoped.prepare(presentation, focus);
        let call_action = self.calls.show(ui, presentation, focus);
        ui.horizontal(|ui| {
            controls::tab_value(
                ui,
                &mut self.kind,
                Kind::Programs,
                "Sources",
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
        framework::line_editor(
            ui,
            "source_context_filter",
            &mut self.filter,
            "Search source / owner / PC",
        );
        let filter = self.filter.trim().to_ascii_lowercase();
        let action = match self.kind {
            Kind::Programs => {
                self.scoped.filter(&filter);
                self.scoped.show(ui, presentation, focus)
            }
            Kind::Accounts => accounts(ui, presentation, focus, selected_account, &filter),
        };
        action.or(call_action)
    }
}

impl Rows {
    fn show(&self, ui: &mut Ui, presentation: Presentation<'_>, focus: &Focus) -> Option<Action> {
        let mut action = None;
        let program = focus.source.as_ref().and_then(|source| source.program());
        egui::ScrollArea::vertical()
            .id_salt("scoped_directory")
            .auto_shrink([false, false])
            .show_rows(ui, 46.0, self.entries.len(), |ui, range| {
                for index in range {
                    ui.push_id(index, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), 46.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| match self.entries[index] {
                                Entry::Group(index) => {
                                    let source = &self.sources[index];
                                    ui.separator();
                                    ui.label(
                                        RichText::new(if source.group_size == 1 {
                                            "SOURCE".into()
                                        } else {
                                            format!("SHARED CODE · {} aliases", source.group_size)
                                        })
                                        .small()
                                        .strong(),
                                    );
                                }
                                Entry::Source(index) => {
                                    let source = &self.sources[index];
                                    if ui
                                        .add(
                                            egui::Button::selectable(
                                                program == Some(source.program),
                                                RichText::new(&source.title).monospace(),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&source.search)
                                        .clicked()
                                        && let Some(source) = presentation.source(source.program)
                                    {
                                        action = Some(Action::Navigate(Intent::Open(
                                            Focus::source(focus.view, source),
                                        )));
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&source.detail)
                                                .small()
                                                .color(palette::MUTED),
                                        )
                                        .truncate(),
                                    );
                                }
                                Entry::Contexts => {
                                    ui.separator();
                                    ui.label(
                                        RichText::new(if program.is_some() {
                                            "SOURCE CONTEXTS"
                                        } else {
                                            "EXECUTION CONTEXTS"
                                        })
                                        .small()
                                        .strong(),
                                    );
                                    if self.contexts.is_empty() {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new("No execution context recorded")
                                                    .small()
                                                    .color(palette::MUTED),
                                            )
                                            .truncate(),
                                        );
                                    }
                                }
                                Entry::Context(index) => {
                                    let context = &self.contexts[index];
                                    let selected = focus.view == context.focus.view
                                        && focus.context == context.focus.context;
                                    if ui
                                        .add(
                                            egui::Button::selectable(
                                                selected,
                                                RichText::new(&context.title).color(
                                                    if context.current {
                                                        palette::TEXT
                                                    } else {
                                                        palette::WARNING
                                                    },
                                                ),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&context.search)
                                        .clicked()
                                    {
                                        action = Some(Action::Navigate(Intent::Open(
                                            context.focus.clone(),
                                        )));
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&context.detail)
                                                .small()
                                                .color(palette::MUTED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&context.search);
                                }
                            },
                        );
                    });
                }
            });
        action
    }

    fn prepare(&mut self, presentation: Presentation<'_>, focus: &Focus) {
        let program = focus.source.as_ref().and_then(|source| source.program());
        let key = (presentation.identity(), focus.view, program);
        if self.key == Some(key) {
            return;
        }
        self.sources.clear();
        self.contexts.clear();
        self.filter = None;
        for (group, aliases) in presentation.index().code_groups().iter().enumerate() {
            for &program in &aliases.programs {
                let Some(source) = presentation.program(program) else {
                    continue;
                };
                let address = source.code_address.as_deref().unwrap_or(
                    if source.kind == evm_abstract_protocol::CodeKind::Initcode {
                        "Constructor code"
                    } else {
                        "Supplied input program"
                    },
                );
                let detail = format!(
                    "{:?} · {} bytes · {} blocks",
                    source.kind,
                    source.bytecode.trim_start_matches("0x").len() / 2,
                    source.blocks.len()
                );
                self.sources.push(Source {
                    program,
                    title: short(address),
                    search: format!(
                        "{address}\n{}\n{:?}\n{detail}",
                        source.code_hash, source.fork
                    )
                    .to_ascii_lowercase(),
                    detail,
                    group,
                    group_size: aliases.programs.len(),
                });
            }
        }
        let contexts = program.map_or_else(
            || presentation.index().contexts(),
            |program| presentation.index().contexts_for_program(program),
        );
        for &target in contexts {
            if target.view.is_minimum() != focus.view.is_minimum()
                || target.view.namespace() != focus.view.namespace()
            {
                continue;
            }
            let Ok(target_focus) = presentation.focus_context(target) else {
                continue;
            };
            let Ok(context) = presentation.context(target) else {
                continue;
            };
            let (title, detail, current, provenance) = match context {
                ResolvedContext::Minimum {
                    procedure,
                    source_status,
                    ..
                } => {
                    let owner = procedure
                        .process
                        .as_ref()
                        .map(|frame| owner(&frame.owner))
                        .unwrap_or_else(|| {
                            procedure
                                .storage_address
                                .clone()
                                .unwrap_or_else(|| "Owner not recorded".into())
                        });
                    let current = source_status == SummarySourceStatus::Current;
                    (
                        format!("{} · {}", short(&owner), mode(Some(procedure.is_static))),
                        format!(
                            "MAY control · {}",
                            match source_status {
                                SummarySourceStatus::Current => "current",
                                SummarySourceStatus::Historical => "historical",
                                SummarySourceStatus::Uncertified => "uncertified",
                            }
                        ),
                        current,
                        format!(
                            "Owner: {owner}\nProcess: {:?}\nInvocation: {:?}",
                            procedure.process, procedure.process_input
                        ),
                    )
                }
                ResolvedContext::Graph {
                    state,
                    frame,
                    coverage,
                    at_current_revision,
                    binding,
                    ..
                } => {
                    let current = coverage == Some(BlockCoverage::Current) && at_current_revision;
                    let location = state
                        .start_pc
                        .map_or_else(|| "No instruction".into(), |pc| format!("PC 0x{pc:04x}"));
                    let is_static = state
                        .process
                        .as_ref()
                        .map(|frame| frame.is_static)
                        .or_else(|| frame.map(|frame| frame.is_static));
                    (
                        format!("{} · {}", short(state.storage_label()), mode(is_static)),
                        format!(
                            "{location} · depth {} · {}{}",
                            state.frame_depth,
                            coverage_label(coverage),
                            if at_current_revision {
                                ""
                            } else {
                                " · earlier input"
                            }
                        ),
                        current,
                        format!(
                            "State: {}\nCode: {}\nOwner: {}\nProcess: {:?}\nJump context: {:?}\nCaller: {:?}\nLogical ADDRESS: {:?}\nRoot binding: {:?}",
                            state.id,
                            state.code_label(),
                            state.storage_label(),
                            state.process,
                            state.context,
                            frame.and_then(|frame| frame.caller.as_ref()),
                            frame.and_then(|frame| frame.address_value.as_ref()),
                            binding,
                        ),
                    )
                }
            };
            let Ok(capabilities) = presentation.capabilities(&target_focus) else {
                continue;
            };
            let search = format!(
                "{title}\n{detail}\n{provenance}\nContext: {:?}\nCode: {}\nCFG: {}\nSSA: {}",
                target_focus.context,
                capability(capabilities.code),
                capability(capabilities.cfg),
                capability(capabilities.ssa)
            );
            self.contexts.push(Context {
                focus: target_focus,
                title,
                detail,
                search,
                current,
            });
        }
        self.key = Some(key);
    }

    fn filter(&mut self, filter: &str) {
        if self.filter.as_deref() == Some(filter) {
            return;
        }
        self.entries.clear();
        let mut group = None;
        for (index, source) in self.sources.iter().enumerate() {
            if !filter.is_empty() && !source.search.contains(filter) {
                continue;
            }
            if group != Some(source.group) {
                self.entries.push(Entry::Group(index));
                group = Some(source.group);
            }
            self.entries.push(Entry::Source(index));
        }
        self.entries.push(Entry::Contexts);
        self.entries.extend(
            self.contexts
                .iter()
                .enumerate()
                .filter(|(_, context)| {
                    filter.is_empty() || context.search.to_ascii_lowercase().contains(filter)
                })
                .map(|(index, _)| Entry::Context(index)),
        );
        self.filter = Some(filter.into());
    }
}

fn accounts(
    ui: &mut Ui,
    presentation: Presentation<'_>,
    focus: &Focus,
    selected_account: Option<&str>,
    filter: &str,
) -> Option<Action> {
    let mut action = None;
    ui.label(
        RichText::new("Original observations")
            .small()
            .color(palette::MUTED),
    );
    egui::ScrollArea::vertical().id_salt("scoped_accounts").auto_shrink([false, false]).show(ui, |ui| {
        if let Some(model) = presentation.report().minimum.as_deref() {
            for (index, account) in model.accounts.iter().enumerate() {
                if !filter.is_empty() && !account.address.to_ascii_lowercase().contains(filter) { continue; }
                let selected = matches!(focus.evidence, EvidenceFocus::Minimum(evidence) if evidence.account == Some(index));
                let title = if account.root_template { "Supplied root state".into() } else { short(&account.address) };
                if ui.selectable_label(selected, RichText::new(title).monospace()).on_hover_text(&account.address).clicked() { action = Some(Action::MinimumAccount(index)); }
                ui.label(RichText::new(format!("{:?} · {} slots", account.code_kind, account.storage.len())).small().color(palette::MUTED));
            }
        } else if let Some(graph) = presentation.report().graph(focus.view.namespace()) {
            for account in &graph.accounts {
                if !filter.is_empty() && !account.address.to_ascii_lowercase().contains(filter) { continue; }
                let root = graph.input_namespace() == Some(account.address.as_str());
                let title = if root { "Supplied root state".into() } else { short(&account.address) };
                if ui.selectable_label(selected_account == Some(account.address.as_str()), RichText::new(title).monospace()).on_hover_text(&account.address).clicked() { action = Some(if root { Action::RootInput } else { Action::Account(account.address.clone()) }); }
                ui.label(RichText::new(format!("{:?} · {:?} · {} slots", account.existence, account.code_kind, account.storage.len())).small().color(palette::MUTED));
            }
        }
    });
    action
}

pub(super) fn owner(owner: &ProcessOwner) -> String {
    match owner {
        ProcessOwner::RootNamespace => "Root input owner".into(),
        ProcessOwner::Named(address) => address.clone(),
        ProcessOwner::Residual => "Residual owner class".into(),
    }
}

fn mode(is_static: Option<bool>) -> &'static str {
    match is_static {
        Some(true) => "static",
        Some(false) => "mutable",
        None => "mode not recorded",
    }
}

fn coverage_label(coverage: Option<BlockCoverage>) -> &'static str {
    match coverage {
        Some(BlockCoverage::Current) => "current receipt",
        Some(BlockCoverage::Stale) => "historical receipt",
        Some(BlockCoverage::Unexecuted) => "unexecuted",
        None => "no receipt",
    }
}

fn capability(capability: Capability) -> &'static str {
    match capability {
        Capability::Available => "ready",
        Capability::Unavailable(reason) => reason.label(),
    }
}

#[cfg(test)]
mod tests;
