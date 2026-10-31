//! Actual entity, source, clone and transform evidence from the core artifact.

use super::{model::ViewState, structured, views::stage_label};
use crate::{
    controls::{self, Density},
    framework, palette,
};
use egui::{RichText, Ui};
use evm_abstract_protocol::decompile_model::{
    Artifact, EntityRef, Stage, TransformEvent, Validation,
};

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
                if let Some(stage) = artifact.stage(selected.stage) {
                    entity_details(ui, stage, selected);
                }
                if let Some(origin) = state.index.origin(artifact, selected) {
                    ui.label(format!("Introduction: {}", origin.reason));
                    if origin.source.is_empty() {
                        ui.label("Generated entity · no original instruction PC");
                    }
                    for span in &origin.source {
                        let label = format!("Original 0x{:04x}..0x{:04x}", span.pc, span.end_pc);
                        let response = controls::action(ui, &label, Density::Regular);
                        controls::observe(ui, &response, &label, "decompile-origin", None);
                        if response.clicked() {
                            state.select_source(artifact, span.pc);
                            state.source_navigation = true;
                        }
                    }
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
                for event in &artifact.events {
                    let label = format!(
                        "Transform {}: {} · {:?}",
                        event.id, event.rule, event.validation.status
                    );
                    let response =
                        controls::tab(ui, state.event == Some(event.id), &label, Density::Regular);
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
                }
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
                    for diagnostic in &artifact.diagnostics {
                        let text = format!(
                            "{:?} · {:?}{}: {}",
                            diagnostic.severity,
                            diagnostic.code,
                            diagnostic.source.map_or_else(String::new, |source| format!(
                                " at 0x{:04x}",
                                source.pc
                            )),
                            diagnostic.message
                        );
                        let response = ui.label(&text);
                        controls::observe(ui, &response, &text, "decompile-diagnostic", None);
                        if let (Some(stage), Some(entity)) = (diagnostic.stage, diagnostic.entity) {
                            link(
                                ui,
                                artifact,
                                state,
                                EntityRef { stage, entity },
                                "Diagnostic entity",
                            );
                        }
                    }
                },
            );
        });
}

fn entity_details(ui: &mut Ui, stage: &Stage, selected: EntityRef) {
    if let Some(block) = stage.ir.block(selected.entity) {
        ui.label(format!(
            "Block b{} · {} operations · memory input m{} · effect input q{}",
            block.id.0,
            block.operations.len(),
            block.memory_parameter.0,
            block.effect_parameter.0
        ));
        ui.monospace(structured::terminator(&block.terminator));
    } else if let Some(operation) = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| operation.id == selected.entity)
    {
        ui.monospace(structured::operation(operation, &stage.ir));
        framework::section(ui, "Primitive effects", |ui| {
            ui.monospace(format!("{:#?}", operation.effects));
        });
    } else if let Some(value) = stage.ir.value(selected.entity) {
        ui.monospace(format!(
            "{:?}\nFact: {:?}\nScope: {:?}",
            value.kind, value.fact, stage.ir.fact_scope
        ));
    } else if let Some(edge) = stage.ir.edge(selected.entity) {
        ui.monospace(format!(
            "e{}: b{} → b{} · {:?}\nArguments: ({})\nMemory: m{} · effect q{}",
            edge.id.0,
            edge.from.0,
            edge.to.0,
            edge.kind,
            structured::values(&edge.arguments),
            edge.memory.0,
            edge.effect.0
        ));
    }
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
        .show_rows(ui, 22.0, references.len(), |ui, range| {
            for position in range {
                ui.push_id(position, |ui| {
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
            for copy in &event.clones {
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
                link(ui, artifact, state, copy.original, "Copy original");
                link(ui, artifact, state, copy.copy, "Copy descendant");
            }
        });
    }
    if !event.redirects.is_empty() {
        framework::section(ui, "Redirected edges", |ui| {
            for redirect in &event.redirects {
                ui.colored_label(
                    palette::WARNING,
                    format!(
                        "Redirected e{}: b{} → b{}",
                        redirect.edge.0, redirect.before.0, redirect.after.0
                    ),
                );
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
    framework::section(ui, "New and removed entities", |ui| {
        for entity in event
            .created
            .iter()
            .filter(|id| !event.clones.iter().any(|copy| copy.copy.entity == **id))
        {
            link(
                ui,
                artifact,
                state,
                EntityRef {
                    stage: event.to_stage,
                    entity: *entity,
                },
                "New",
            );
        }
        if let Some(stage) = event.from_stage {
            for entity in &event.deleted {
                link(
                    ui,
                    artifact,
                    state,
                    EntityRef {
                        stage,
                        entity: *entity,
                    },
                    "Removed",
                );
            }
        }
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
