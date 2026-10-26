//! Call links retain exact source/context targets and let the shell own history.

use super::{Action, short};
use crate::{
    app::model::{
        ReportViewId,
        navigation::{ContextFocus, Focus, Intent},
        presentation::{ContextTarget, Presentation, PublicationIdentity, ResolvedContext},
    },
    palette,
};
use evm_abstract_protocol::{EdgeKind, ProcessSource};

#[derive(Default)]
pub(super) struct Calls {
    key: Option<(PublicationIdentity, ReportViewId, Option<ContextFocus>)>,
    links: Vec<Link>,
}

struct Link {
    focus: Focus,
    title: String,
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
                                if ui
                                    .add(egui::Button::new(&link.title).truncate())
                                    .on_hover_text(&link.detail)
                                    .clicked()
                                {
                                    action =
                                        Some(Action::Navigate(Intent::Open(link.focus.clone())));
                                }
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&link.detail)
                                            .small()
                                            .color(palette::MUTED),
                                    )
                                    .truncate(),
                                );
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
        let (code, owner) = match context {
            ResolvedContext::Minimum { procedure, .. } => (
                procedure.code_address.clone().unwrap_or_else(|| {
                    source_name(procedure.process.as_ref().map(|frame| &frame.source))
                }),
                procedure
                    .process
                    .as_ref()
                    .map(|frame| super::scoped::owner(&frame.owner))
                    .or_else(|| procedure.storage_address.clone())
                    .unwrap_or_else(|| "Owner not recorded".into()),
            ),
            ResolvedContext::Graph { state, .. } => {
                (state.code_label().into(), state.storage_label().into())
            }
        };
        self.links.push(Link {
            title: format!("{verb} {}", short(&code)),
            detail: format!("Code: {code} · owner: {owner} · {:?}", target.context),
            focus,
        });
    }
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
