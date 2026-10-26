//! Call links retain exact source/context targets and let the shell own history.

use super::Action;
use crate::{
    app::model::{
        ReportViewId,
        navigation::{ContextFocus, Focus, Intent},
        presentation::{ContextTarget, Presentation, PublicationIdentity, ResolvedContext},
    },
    controls::{self, Density},
    palette,
};
use evm_abstract_protocol::{CodeKind, EdgeKind, ProcessSource};

#[derive(Default)]
pub(super) struct Calls {
    key: Option<(PublicationIdentity, ReportViewId, Option<ContextFocus>)>,
    links: Vec<Link>,
}

struct Link {
    focus: Focus,
    title: String,
    owner: String,
    detail: String,
}

impl Calls {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        presentation: Presentation<'_>,
        focus: &Focus,
    ) -> Option<Action> {
        self.prepare(presentation, focus);
        if self.links.is_empty() {
            return None;
        }
        let mut action = None;
        egui::CollapsingHeader::new(format!("Context calls · {} targets", self.links.len()))
            .id_salt("context_call_targets")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("context_calls_scroll")
                    .max_height(120.0)
                    .show_rows(ui, 42.0, self.links.len(), |ui, range| {
                        for index in range {
                            let link = &self.links[index];
                            ui.push_id(index, |ui| {
                                action = show_link(ui, link).or(action.take());
                            });
                        }
                    });
            });
        action
    }

    fn prepare(&mut self, presentation: Presentation<'_>, focus: &Focus) {
        if presentation.validate(focus).is_err() {
            self.key = None;
            self.links.clear();
            return;
        }
        let key = (presentation.identity(), focus.view, focus.context);
        if self.key == Some(key) {
            return;
        }
        self.links.clear();
        let Some(context) = focus.context else {
            self.key = Some(key);
            return;
        };
        let Ok(context) = presentation.context(ContextTarget {
            view: focus.view,
            context,
        }) else {
            return;
        };
        match context {
            ResolvedContext::Minimum { procedure, .. } => {
                let Some(ContextFocus::Minimum { summary, .. }) = focus.context else {
                    return;
                };
                for &procedure in &procedure.dependencies {
                    self.add(
                        presentation,
                        ContextTarget {
                            view: focus.view,
                            context: ContextFocus::Minimum { summary, procedure },
                        },
                        "Callee",
                    );
                }
            }
            ResolvedContext::Graph { graph, state, .. } => {
                for edge in graph.edges.iter().filter(|edge| edge.from == state.id) {
                    let verb = match edge.kind {
                        EdgeKind::Call => "Enter",
                        EdgeKind::Return | EdgeKind::SummaryReturn => "Return to",
                        EdgeKind::Revert | EdgeKind::SummaryRevert => "Revert to",
                        EdgeKind::Failure | EdgeKind::SummaryFailure => "Failure to",
                        _ => continue,
                    };
                    self.add(
                        presentation,
                        ContextTarget {
                            view: focus.view,
                            context: ContextFocus::Graph { state: edge.to },
                        },
                        verb,
                    );
                }
            }
        }
        self.key = Some(key);
    }

    fn add(&mut self, presentation: Presentation<'_>, target: ContextTarget, verb: &str) {
        let Ok(focus) = presentation.focus_context(target) else {
            return;
        };
        let Ok(context) = presentation.context(target) else {
            return;
        };
        let (code, owner, source, kind) = match context {
            ResolvedContext::Minimum { procedure, .. } => {
                let source = procedure.process.as_ref().map(|frame| &frame.source);
                (
                    source.map_or_else(
                        || {
                            procedure
                                .code_address
                                .clone()
                                .unwrap_or_else(|| "Uncaptured source".into())
                        },
                        |source| source_name(Some(source)),
                    ),
                    procedure
                        .process
                        .as_ref()
                        .map(|frame| super::scoped::owner(&frame.owner))
                        .or_else(|| procedure.storage_address.clone())
                        .unwrap_or_else(|| "Owner not recorded".into()),
                    source,
                    Some(procedure.kind),
                )
            }
            ResolvedContext::Graph { state, frame, .. } => (
                state.code_label().into(),
                state.storage_label().into(),
                state.process.as_ref().map(|frame| &frame.source),
                frame.map(|frame| frame.kind).or_else(|| {
                    state
                        .program
                        .and_then(|id| presentation.program(id))
                        .map(|program| program.kind)
                }),
            ),
        };
        let label = source_label(source, kind, &code);
        let title = match verb {
            "Callee" => label,
            "Return to" => format!("Return · {label}"),
            "Revert to" => format!("Revert · {label}"),
            "Failure to" => format!("Failure · {label}"),
            _ => format!("{verb} · {label}"),
        };
        self.links.push(Link {
            detail: format!(
                "Target: {title}\nCode: {code}\nOwner: {owner}\nAction: {verb}\nSource: {source:?}\nContext: {:?}",
                target.context
            ),
            title,
            owner,
            focus,
        });
    }
}

fn show_link(ui: &mut egui::Ui, link: &Link) -> Option<Action> {
    let mut action = None;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 42.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let width = (ui.available_width() - 12.0).max(0.0);
            let title = fitted_label(ui, &link.title, width);
            // Fit the identity once. A second end truncation would hide the suffix.
            if controls::action(ui, title, Density::Compact)
                .on_hover_text(&link.detail)
                .clicked()
            {
                action = Some(Action::Navigate(Intent::Open(link.focus.clone())));
            }
            let owner = match link.owner.as_str() {
                "Root input owner" => "Root input",
                "Residual owner class" => "Residual",
                value => value,
            };
            let caption = fitted_label(ui, &format!("Owner ▾ {owner}"), width);
            let response = controls::menu_button(
                ui,
                egui::RichText::new(caption).color(palette::MUTED),
                Density::Inline,
                |ui| {
                    ui.set_min_width(240.0);
                    ui.set_max_width(360.0);
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(&link.detail).selectable(true).wrap());
                        });
                    if controls::action(ui, "Copy full target", Density::Compact).clicked() {
                        ui.ctx().copy_text(link.detail.clone());
                    }
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    format!("Details for {}. Owner: {}", link.title, link.owner),
                )
            });
            response.on_hover_text(&link.detail);
        },
    );
    action
}

fn source_label(source: Option<&ProcessSource>, kind: Option<CodeKind>, code: &str) -> String {
    match source {
        Some(ProcessSource::Precompile(source)) => precompile_label(&source.address),
        Some(ProcessSource::AnyRuntime(_)) => "Unknown external".into(),
        Some(ProcessSource::InputProgram(_)) => "Supplied input program".into(),
        Some(ProcessSource::InitCode(_) | ProcessSource::AnyInitCode(_)) => {
            "Constructor code".into()
        }
        Some(ProcessSource::Empty(_)) => "Empty execution".into(),
        Some(ProcessSource::InputDelegation(_)) => format!("Delegated {code}"),
        Some(ProcessSource::InvalidDelegation(_)) => format!("Invalid delegation {code}"),
        Some(ProcessSource::Runtime(_)) => format!("Runtime {code}"),
        None => match kind {
            Some(CodeKind::Precompile) => precompile_label(code),
            Some(CodeKind::Unknown | CodeKind::AbstractEnvironment) => "Unknown external".into(),
            Some(CodeKind::Runtime) => format!("Runtime {code}"),
            _ => code.into(),
        },
    }
}

fn precompile_label(address: &str) -> String {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).map_or_else(
        |_| format!("Precompile {address}"),
        |number| format!("Precompile {number}"),
    )
}

/// Size only visible labels, retaining the address tail within the actual row.
fn fitted_label(ui: &egui::Ui, label: &str, width: f32) -> String {
    let fits = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.into(), egui::FontId::proportional(13.0), palette::TEXT)
            .size()
            .x
            <= width
    };
    if fits(label) {
        return label.into();
    }
    if let Some(start) = label.rfind("0x") {
        let digits = &label[start + 2..];
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            let action = label
                .split_once(" · ")
                .map(|(verb, _)| format!("{verb} · "));
            for length in (4..=digits.len().min(8)).rev() {
                for prefix in [&label[..start], action.as_deref().unwrap_or(""), ""] {
                    let candidate = format!("{prefix}…{}", &digits[digits.len() - length..]);
                    if fits(&candidate) {
                        return candidate;
                    }
                }
            }
        }
    }
    if let Some((_, target)) = label.split_once(" · ")
        && fits(target)
    {
        return target.into();
    }
    let chars = label.chars().collect::<Vec<_>>();
    for length in (1..chars.len()).rev() {
        let candidate = format!("{}…", chars[..length].iter().collect::<String>());
        if fits(&candidate) {
            return candidate;
        }
    }
    "…".into()
}

fn source_name(source: Option<&ProcessSource>) -> String {
    match source {
        Some(ProcessSource::InputProgram(_)) => "Supplied input program".into(),
        Some(ProcessSource::Runtime(source) | ProcessSource::InvalidDelegation(source)) => {
            source.source.clone()
        }
        Some(ProcessSource::InputDelegation(source)) => source.target.clone(),
        Some(ProcessSource::InitCode(_) | ProcessSource::AnyInitCode(_)) => {
            "Constructor code".into()
        }
        Some(ProcessSource::AnyRuntime(_)) => "Unknown external code".into(),
        Some(ProcessSource::Empty(_)) => "Empty execution".into(),
        Some(ProcessSource::Precompile(source)) => source.address.clone(),
        None => "Uncaptured source".into(),
    }
}

#[cfg(test)]
mod tests;
