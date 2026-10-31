//! Stage controls, structured code, immutable graph/IR comparisons and source
//! navigation. Both graph panes call the same neutral canvas implementation.

use super::{
    diff::Differences,
    model::{EntityKind, ViewState},
    rows, structured,
};
use crate::{
    controls::{self, Density},
    palette,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::decompile_model::{
    Artifact, ArtifactId, EntityRef, Stage, StageId, StageKind,
};
use std::collections::BTreeSet;

mod graph;
mod ir;
mod source;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Route {
    #[default]
    Structured,
    Graphs,
    Ir,
    Changes,
    Source,
}
impl Route {
    fn label(self) -> &'static str {
        match self {
            Self::Structured => "Structured code",
            Self::Graphs => "Graphs",
            Self::Ir => "IR",
            Self::Changes => "Changes",
            Self::Source => "Original bytecode",
        }
    }
}

#[derive(Default)]
pub(super) struct Views {
    route: Route,
    before_graph: graph::Graph,
    current_graph: graph::Graph,
    before_ir: ir::Ir,
    current_ir: ir::Ir,
    comparison: Option<(ArtifactId, StageId, StageId)>,
    pub differences: Differences,
    rendered: Option<(ArtifactId, StageId, Vec<structured::Line>)>,
    narrow_before: bool,
    source: source::Source,
}

impl Views {
    pub fn route_label(&self) -> &'static str {
        self.route.label()
    }
    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        self.before_graph.prepare_input(input);
        self.current_graph.prepare_input(input);
    }

    fn compare(&mut self, artifact: &Artifact, state: &ViewState) {
        if let (Some(before), Some(after)) = (state.before, state.current) {
            let identity = (artifact.id.clone(), before, after);
            if self.comparison.as_ref() != Some(&identity) {
                self.differences = Differences::new(artifact, before, after);
                self.comparison = Some(identity);
            }
        }
    }

    pub fn controls(&mut self, ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Current stage");
            let mut current = state.current;
            let selected = current
                .and_then(|id| artifact.stage(id))
                .map(stage_label)
                .unwrap_or_else(|| "No stage".into());
            let combo = egui::ComboBox::from_id_salt("decompile_current_stage")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for stage in &artifact.stages {
                        controls::tab_value(
                            ui,
                            &mut current,
                            Some(stage.id),
                            stage_label(stage),
                            Density::Regular,
                        );
                    }
                });
            controls::observe(ui, &combo.response, "Current stage", "combobox", None);
            if current != state.current
                && let Some(id) = current
            {
                state.set_stage(artifact, id);
            }
            ui.label("Before stage");
            let ancestors = ancestors(artifact, state.current);
            let selected = state
                .before
                .and_then(|id| artifact.stage(id))
                .map(stage_label)
                .unwrap_or_else(|| "No stage".into());
            let combo = egui::ComboBox::from_id_salt("decompile_before_stage")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for stage in artifact
                        .stages
                        .iter()
                        .filter(|stage| ancestors.contains(&stage.id))
                    {
                        controls::tab_value(
                            ui,
                            &mut state.before,
                            Some(stage.id),
                            stage_label(stage),
                            Density::Regular,
                        );
                    }
                });
            controls::observe(ui, &combo.response, "Before stage", "combobox", None);
            if let Some(stage) = artifact.current() {
                ui.label(
                    RichText::new(format!("Last validated: {} {:?}", stage.id.0, stage.kind))
                        .small()
                        .color(palette::MUTED),
                );
            }
            if let Some(stage) = state.current.and_then(|id| artifact.stage(id)) {
                ui.label(format!("Facts: {:?}", stage.ir.fact_scope));
            }
        });
        ui.horizontal_wrapped(|ui| {
            for route in [
                Route::Structured,
                Route::Graphs,
                Route::Ir,
                Route::Changes,
                Route::Source,
            ] {
                controls::tab_value(ui, &mut self.route, route, route.label(), Density::Regular);
            }
        });
        self.compare(artifact, state);
    }

    pub fn show(&mut self, ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
        self.compare(artifact, state);
        if std::mem::take(&mut state.compare_navigation) {
            self.route = Route::Changes;
        } else if state.source_navigation {
            self.route = Route::Source;
        } else if state.entity_navigation && self.route == Route::Source {
            self.route = Route::Ir;
        }
        if state.entity_navigation
            && let Some(selected) = &state.selected
        {
            let at = selected.at;
            let keep_code = self.route == Route::Structured
                && state.index.entities.get(&at) == Some(&EntityKind::Operation)
                && artifact
                    .stage(at.stage)
                    .is_some_and(|stage| stage.ir.structured.is_some());
            if keep_code {
                self.prepare_code(
                    artifact,
                    artifact.stage(at.stage).expect("selected stage"),
                    state,
                );
            }
            self.route = match state.index.entities.get(&selected.at) {
                Some(EntityKind::Structure | EntityKind::Transition) => Route::Structured,
                Some(EntityKind::Operation)
                    if keep_code
                        && self.rendered.as_ref().is_some_and(|(_, _, lines)| {
                            lines.iter().any(|line| line.entity == at.entity)
                        }) =>
                {
                    Route::Structured
                }
                _ => Route::Ir,
            };
        }
        match self.route {
            Route::Structured => self.code(ui, artifact, state),
            Route::Graphs => self.graphs(ui, artifact, state),
            Route::Ir => self.ir(ui, artifact, state),
            Route::Changes => changes(ui, artifact, state, &self.differences),
            Route::Source => self.source.show(ui, artifact, state),
        }
    }

    fn code(&mut self, ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
        let Some(stage) = state.current.and_then(|id| artifact.stage(id)) else {
            crate::widgets::empty(
                ui,
                "No stage was produced; inspect diagnostics for this attempt",
            );
            return;
        };
        if stage.kind == StageKind::Decoded {
            ui.label("Recovery unavailable for this stage. Showing original disassembly; recovered SSA is not available.");
            self.source.show(ui, artifact, state);
            return;
        }
        if stage.ir.structured.is_none() {
            ui.label(format!(
                "{} has no structured representation. Select a Structured stage or inspect its IR.",
                stage_label(stage)
            ));
            self.current_ir
                .show(ui, artifact, stage, state, &self.differences, false);
            return;
        }
        self.prepare_code(artifact, stage, state);
        let lines = &self.rendered.as_ref().expect("rendered stage").2;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{} · {:?}",
                stage_label(stage),
                stage.validation.status
            ));
            if controls::action(ui, "Copy structured text", Density::Compact).clicked() {
                ui.ctx().copy_text(
                    lines
                        .iter()
                        .map(|line| format!("{}{}", "    ".repeat(line.indent), line.text))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
        });
        let mut scroll = egui::ScrollArea::both().id_salt(("decompile_structured", stage.id.0));
        if state.entity_navigation
            && let Some(selected) = state.selected_in(artifact, stage.id)
            && let Some(position) = lines.iter().position(|line| line.entity == selected)
        {
            scroll = scroll.vertical_scroll_offset(rows::offset(ui, position));
        }
        scroll.show_rows(ui, rows::HEIGHT, lines.len(), |ui, range| {
            for number in range {
                let line = &lines[number];
                rows::row(ui, number, |ui| {
                    let at = EntityRef {
                        stage: stage.id,
                        entity: line.entity,
                    };
                    let text = format!("{}{}", "    ".repeat(line.indent), line.text);
                    let visible = entity_line(
                        ui,
                        artifact,
                        state,
                        at,
                        &text,
                        "decompile-code",
                        &self.differences.markers(line.entity, false),
                    );
                    if visible && state.selected_in(artifact, stage.id) == Some(line.entity) {
                        state.entity_navigation = false;
                    }
                });
            }
        });
    }

    fn prepare_code(&mut self, artifact: &Artifact, stage: &Stage, state: &ViewState) {
        let key = (artifact.id.clone(), stage.id);
        if self
            .rendered
            .as_ref()
            .is_none_or(|(artifact, stage, _)| (artifact, *stage) != (&key.0, key.1))
        {
            self.rendered = Some((
                key.0,
                key.1,
                structured::lines_indexed(stage, &state.index.stage(stage.id)),
            ));
        }
    }

    fn comparison_side(&mut self, ui: &mut Ui, left: &str, right: &str) -> bool {
        let wide = ui.available_width() >= 840.0;
        if !wide {
            ui.horizontal(|ui| {
                controls::tab_value(ui, &mut self.narrow_before, true, left, Density::Regular);
                controls::tab_value(ui, &mut self.narrow_before, false, right, Density::Regular);
            });
        }
        wide
    }

    fn graphs(&mut self, ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
        let before = state.before.and_then(|id| artifact.stage(id));
        let after = state.current.and_then(|id| artifact.stage(id));
        ui.label(self.differences.summary());
        let wide = self.comparison_side(ui, "Before graph", "Current graph");
        if wide {
            ui.columns(2, |columns| {
                if let Some(stage) = before {
                    self.before_graph.show(
                        &mut columns[0],
                        artifact,
                        stage,
                        state,
                        &self.differences,
                        true,
                    );
                }
                if let Some(stage) = after {
                    self.current_graph.show(
                        &mut columns[1],
                        artifact,
                        stage,
                        state,
                        &self.differences,
                        false,
                    );
                }
            });
        } else if self.narrow_before {
            if let Some(stage) = before {
                self.before_graph
                    .show(ui, artifact, stage, state, &self.differences, true);
            }
        } else if let Some(stage) = after {
            self.current_graph
                .show(ui, artifact, stage, state, &self.differences, false);
        }
    }

    fn ir(&mut self, ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
        let before = state.before.and_then(|id| artifact.stage(id));
        let after = state.current.and_then(|id| artifact.stage(id));
        ui.label(self.differences.summary());
        let wide = self.comparison_side(ui, "Before IR", "Current IR");
        if wide {
            ui.columns(2, |columns| {
                if let Some(stage) = before {
                    self.before_ir.show(
                        &mut columns[0],
                        artifact,
                        stage,
                        state,
                        &self.differences,
                        true,
                    );
                }
                if let Some(stage) = after {
                    self.current_ir.show(
                        &mut columns[1],
                        artifact,
                        stage,
                        state,
                        &self.differences,
                        false,
                    );
                }
            });
        } else if self.narrow_before {
            if let Some(stage) = before {
                self.before_ir
                    .show(ui, artifact, stage, state, &self.differences, true);
            }
        } else if let Some(stage) = after {
            self.current_ir
                .show(ui, artifact, stage, state, &self.differences, false);
        }
    }
}

pub(super) fn stage_label(stage: &Stage) -> String {
    format!(
        "{} · {:?} · {:?}",
        stage.id.0, stage.kind, stage.validation.status
    )
}

fn ancestors(artifact: &Artifact, current: Option<StageId>) -> BTreeSet<StageId> {
    let mut found = BTreeSet::new();
    let mut cursor = current;
    while let Some(id) = cursor {
        if !found.insert(id) {
            break;
        }
        cursor = artifact.stage(id).and_then(|stage| stage.parent);
    }
    found
}

pub(super) fn entity_line(
    ui: &mut Ui,
    artifact: &Artifact,
    state: &mut ViewState,
    at: EntityRef,
    text: &str,
    role: &'static str,
    markers: &[(String, egui::Color32)],
) -> bool {
    ui.horizontal(|ui| {
        for (label, color) in markers {
            ui.colored_label(*color, label);
        }
        let selected = state.selected_in(artifact, at.stage) == Some(at.entity);
        let response = ui.add(
            egui::Label::new(RichText::new(text).monospace().color(if selected {
                palette::ACCENT
            } else {
                palette::TEXT
            }))
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(egui::Sense::click()),
        );
        let accessible_label = if markers.is_empty() {
            text.to_owned()
        } else {
            format!(
                "{} · {text}",
                markers
                    .iter()
                    .map(|(label, _)| label.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        };
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                selected,
                &accessible_label,
            )
        });
        controls::observe(ui, &response, &accessible_label, role, Some(selected));
        if response.clicked() {
            state.select(artifact, at);
        }
        rows::visible(ui, &response)
    })
    .inner
}

fn changes(ui: &mut Ui, artifact: &Artifact, state: &mut ViewState, differences: &Differences) {
    ui.strong("Before / after differences");
    ui.label(differences.summary());
    ui.label("New · Copied · Removed · Redirected · Changed");
    egui::ScrollArea::vertical()
        .id_salt("decompile_changes")
        .show_rows(ui, rows::HEIGHT, differences.rows.len(), |ui, range| {
            for position in range {
                let (before, entity) = differences.rows[position];
                let Some(stage) = (if before { state.before } else { state.current }) else {
                    continue;
                };
                let at = EntityRef { stage, entity };
                if !state.index.contains(at) {
                    continue;
                }
                let labels = differences.markers(entity, before);
                rows::row(ui, (before, entity.0), |ui| {
                    let text = format!(
                        "{} entity {} · stage {} · {:?}",
                        if before { "Before" } else { "After" },
                        entity.0,
                        stage.0,
                        state.index.entities.get(&at)
                    );
                    entity_line(
                        ui,
                        artifact,
                        state,
                        at,
                        &text,
                        "decompile-difference",
                        &labels,
                    );
                });
            }
        });
}
