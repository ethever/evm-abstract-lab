//! Actual entity, source, clone and transform evidence from the core artifact.

use super::{model::ViewState, rows, views::stage_label};
use crate::{
    controls::{self, Density},
    framework, palette,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::decompile_model::{Artifact, EntityRef, TransformEvent, Validation};
#[cfg(test)]
mod tests;

pub(super) fn show(ui: &mut Ui, artifact: &Artifact, state: &mut ViewState) {
    ui.strong("Transform inspector");
    egui::ScrollArea::vertical()
        .id_salt("decompile_inspector_scroll")
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("Artifact {}", artifact.id.0))
                    .small()
                    .color(palette::MUTED),
            );
            if let Some(selected) = state.selected.as_ref().map(|selected| selected.at) {
                ui.strong(format!(
                    "Stage {} · entity {} · {:?}",
                    selected.stage.0,
                    selected.entity.0,
                    state.index.entities.get(&selected)
                ));
                let details = state.details(artifact, selected);
                for text in &details.lines {
                    let response = ui.label(text);
                    controls::observe(ui, &response, text, "decompile-entity-detail", None);
                }
                framework::section(ui, "Entity references", |ui| {
                    links(ui, artifact, state, &details.references, "Reference")
                });
                if let Some(origin) = state.index.origin(artifact, selected) {
                    ui.label(format!("Introduction: {}", origin.reason));
                    if origin.source.is_empty() {
                        ui.label("Generated entity · no original instruction PC");
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("original_spans", selected.stage.0, selected.entity.0))
                        .max_height(100.0)
                        .show_rows(ui, rows::HEIGHT, origin.source.len(), |ui, range| {
                            for position in range {
                                rows::row(ui, position, |ui| {
                                    let span = origin.source[position];
                                    let label = format!(
                                        "Original 0x{:04x}..0x{:04x}",
                                        span.pc, span.end_pc
                                    );
                                    let response = controls::action(ui, &label, Density::Regular);
                                    controls::observe(
                                        ui,
                                        &response,
                                        &label,
                                        "decompile-origin",
                                        None,
                                    );
                                    if response.clicked() {
                                        state.select_source(artifact, span.pc);
                                        state.source_navigation = true;
                                    }
                                });
                            }
                        });
                    framework::section(ui, "Parent occurrences", |ui| {
                        links(ui, artifact, state, &origin.parents, "Parent");
                    });
                    framework::section(ui, "Entity descendants", |ui| {
                        let descendants = state.entity_descendants(artifact, selected);
                        ui.label(format!("{} descendant occurrences", descendants.len()));
                        links(ui, artifact, state, &descendants, "Descendant");
                    });
                } else {
                    ui.colored_label(palette::WARNING, "No origin record for this occurrence");
                }
            }
            if let Some(pc) = state.source_pc {
                ui.separator();
                ui.strong(format!("Original instruction 0x{pc:04x}"));
                let descendants = state.source_descendants(artifact, pc);
                ui.label(format!("All source descendants ({})", descendants.len()));
                links(ui, artifact, state, &descendants, "Source descendant");
            }
            ui.separator();
            framework::section(ui, "Transformation history", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("transform_history")
                    .max_height(160.0)
                    .show_rows(ui, rows::HEIGHT, artifact.events.len(), |ui, range| {
                        for position in range {
                            rows::row(ui, position, |ui| {
                                let event = &artifact.events[position];
                                let label = format!(
                                    "Transform {}: {} · {:?}",
                                    event.id, event.rule, event.validation.status
                                );
                                let response = controls::tab(
                                    ui,
                                    state.event == Some(event.id),
                                    &label,
                                    Density::Regular,
                                );
                                controls::observe(
                                    ui,
                                    &response,
                                    &label,
                                    "decompile-transform",
                                    Some(state.event == Some(event.id)),
                                );
                                if response.clicked() {
                                    state.event = Some(event.id);
                                }
                            });
                        }
                    });
            });
            if let Some(event) = state
                .event
                .and_then(|id| artifact.events.iter().find(|event| event.id == id))
            {
                transform(ui, artifact, state, event);
            }
            if let Some(stage) = state.current.and_then(|id| artifact.stage(id)) {
                framework::section(ui, "Stage validation", |ui| {
                    ui.label(stage_label(stage));
                    validation(ui, &stage.validation);
                });
            }
            framework::section(
                ui,
                &format!("Diagnostics ({})", artifact.diagnostics.len()),
                |ui| {
                    if artifact.diagnostics.is_empty() {
                        ui.label("No recovery or transformation diagnostics");
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("decompile_diagnostics")
                        .max_height(160.0)
                        .show_rows(
                            ui,
                            rows::HEIGHT,
                            artifact.diagnostics.len() * 2,
                            |ui, range| {
                                for position in range {
                                    rows::row(ui, position, |ui| {
                                        let diagnostic = &artifact.diagnostics[position / 2];
                                        if position % 2 == 0 {
                                            let text = format!(
                                                "{:?} · {:?}: {}",
                                                diagnostic.severity,
                                                diagnostic.code,
                                                diagnostic.message
                                            );
                                            let response = ui
                                                .add(
                                                    egui::Label::new(&text)
                                                        .wrap_mode(egui::TextWrapMode::Extend),
                                                )
                                                .on_hover_text(&text);
                                            controls::observe(
                                                ui,
                                                &response,
                                                &text,
                                                "decompile-diagnostic",
                                                None,
                                            );
                                        } else if let (Some(stage), Some(entity)) =
                                            (diagnostic.stage, diagnostic.entity)
                                        {
                                            link(
                                                ui,
                                                artifact,
                                                state,
                                                EntityRef { stage, entity },
                                                "Diagnostic entity",
                                            );
                                        }
                                    });
                                }
                            },
                        );
                },
            );
        });
}

fn link(ui: &mut Ui, artifact: &Artifact, state: &mut ViewState, at: EntityRef, prefix: &str) {
    let label = format!(
        "{prefix} s{}/e{} · {:?}",
        at.stage.0,
        at.entity.0,
        state.index.entities.get(&at)
    );
    let response = controls::action_enabled(ui, state.index.contains(at), &label, Density::Compact);
    controls::observe(ui, &response, &label, "decompile-entity-link", None);
    if response.clicked() {
        state.navigate(artifact, at);
    }
}

fn links(
    ui: &mut Ui,
    artifact: &Artifact,
    state: &mut ViewState,
    references: &[EntityRef],
    prefix: &str,
) {
    egui::ScrollArea::vertical()
        .id_salt(("entity_links", prefix))
        .max_height(160.0)
        .show_rows(ui, rows::HEIGHT, references.len(), |ui, range| {
            for position in range {
                rows::row(ui, position, |ui| {
                    link(ui, artifact, state, references[position], prefix)
                });
            }
        });
}

fn transform(ui: &mut Ui, artifact: &Artifact, state: &mut ViewState, event: &TransformEvent) {
    let title = format!("Transform {} · {}", event.id, event.rule);
    let response = ui.strong(&title);
    controls::observe(ui, &response, &title, "decompile-transform-detail", None);
    let response = ui.label(&event.reason);
    controls::observe(
        ui,
        &response,
        &event.reason,
        "decompile-transform-reason",
        None,
    );
    ui.label(format!(
        "Stage {} → {}",
        event
            .from_stage
            .map_or_else(|| "source input".into(), |id| id.0.to_string()),
        event.to_stage.0
    ));
    validation(ui, &event.validation);
    let comparable = event
        .from_stage
        .is_some_and(|id| artifact.stage(id).is_some())
        && artifact.stage(event.to_stage).is_some();
    if controls::action_enabled(ui, comparable, "Compare transform stages", Density::Regular)
        .clicked()
    {
        state.set_stage(artifact, event.to_stage);
        state.before = event.from_stage;
        state.compare_navigation = true;
    }
    ui.label(format!(
        "New: {} · Copied: {} · Removed: {} · Redirected: {}",
        event.created.len().saturating_sub(event.clones.len()),
        event.clones.len(),
        event.deleted.len(),
        event.redirects.len()
    ));
    framework::section(ui, "Transform inputs", |ui| {
        links(ui, artifact, state, &event.inputs, "Input")
    });
    framework::section(ui, "Transform outputs", |ui| {
        links(ui, artifact, state, &event.outputs, "Output")
    });
    if !event.clones.is_empty() {
        framework::section(ui, "Copied entities", |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("event_clones", event.id))
                .max_height(180.0)
                .show_rows(ui, rows::HEIGHT, event.clones.len() * 3, |ui, range| {
                    for position in range {
                        rows::row(ui, position, |ui| {
                            let copy = &event.clones[position / 3];
                            match position % 3 {
                                0 => {
                                    ui.colored_label(
                                        palette::PURPLE,
                                        format!(
                                            "Copied s{}/e{} → s{}/e{}",
                                            copy.original.stage.0,
                                            copy.original.entity.0,
                                            copy.copy.stage.0,
                                            copy.copy.entity.0
                                        ),
                                    );
                                }
                                1 => link(ui, artifact, state, copy.original, "Copy original"),
                                _ => link(ui, artifact, state, copy.copy, "Copy descendant"),
                            }
                        });
                    }
                });
        });
    }
    if !event.redirects.is_empty() {
        framework::section(ui, "Redirected edges", |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("event_redirects", event.id))
                .max_height(160.0)
                .show_rows(ui, rows::HEIGHT, event.redirects.len() * 2, |ui, range| {
                    for position in range {
                        rows::row(ui, position, |ui| {
                            let redirect = &event.redirects[position / 2];
                            if position % 2 == 0 {
                                ui.colored_label(
                                    palette::WARNING,
                                    format!(
                                        "Redirected e{}: b{} → b{}",
                                        redirect.edge.0, redirect.before.0, redirect.after.0
                                    ),
                                );
                            } else {
                                link(
                                    ui,
                                    artifact,
                                    state,
                                    EntityRef {
                                        stage: event.to_stage,
                                        entity: redirect.edge,
                                    },
                                    "Redirected edge",
                                );
                            }
                        });
                    }
                });
        });
    }
    let created = state.new_entities(event);
    framework::section(ui, "New and removed entities", |ui| {
        let removed = if event.from_stage.is_some() {
            event.deleted.len()
        } else {
            0
        };
        egui::ScrollArea::vertical()
            .id_salt(("event_entities", event.id))
            .max_height(160.0)
            .show_rows(ui, rows::HEIGHT, created.len() + removed, |ui, range| {
                for position in range {
                    rows::row(ui, position, |ui| {
                        let (stage, entity, prefix) = if position < created.len() {
                            (event.to_stage, created[position], "New")
                        } else {
                            (
                                event.from_stage.expect("removed stage"),
                                event.deleted[position - created.len()],
                                "Removed",
                            )
                        };
                        link(ui, artifact, state, EntityRef { stage, entity }, prefix);
                    });
                }
            });
    });
}

fn validation(ui: &mut Ui, validation: &Validation) {
    ui.label(format!("Validation: {:?}", validation.status));
    for check in &validation.checks {
        ui.label(format!(
            "{} · {}: {}",
            if check.passed { "Passed" } else { "Failed" },
            check.name,
            check.message
        ));
    }
}
