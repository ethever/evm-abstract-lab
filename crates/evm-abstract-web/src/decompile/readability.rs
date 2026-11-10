//! Source-qualified reading preferences. Reanalysis always uses the published
//! input and recovery policy; presentation IDs never become backend overrides.

use super::model::{Selection, ViewState};
use crate::{
    controls::{self, Density},
    framework, palette,
};
use evm_abstract_protocol::{
    DecompileRequest, Fork,
    decompile_model::{
        Artifact, ArtifactId, EntityId, ProcedurePreference, ReadabilityConfig, ReadabilityMetrics,
        ReadabilityProfile, ReadabilityReason, RegionAnchor, RegionDecision, RegionPreference,
        StructureForm, StructurePreference, readability_source,
    },
};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

#[derive(Default)]
pub(super) struct Preferences {
    source: Option<ArtifactId>,
    config: ReadabilityConfig,
    region: Option<RegionAnchor>,
    seen_selection: Option<Selection>,
    notice: Option<&'static str>,
}

impl Preferences {
    pub fn install(&mut self, artifact: &Artifact, state: &ViewState) {
        let source = artifact.readability_source();
        let changed = self.source.as_ref().is_some_and(|prior| prior != &source);
        let selection_lost = self.region.as_ref().is_some_and(|region| {
            !artifact
                .readability
                .iter()
                .any(|decision| &decision.region == region)
        });
        self.notice = if changed && self.region.is_some() {
            Some("Region selection cleared: the input or recovery settings changed.")
        } else if selection_lost {
            Some("The previous region is absent from this result; select a current region.")
        } else {
            None
        };
        if changed || selection_lost {
            self.region = None;
        }
        self.source = Some(source);
        self.config = artifact.config.readability.clone();
        let previous_count = self.config.regions.len();
        self.config.regions.retain(|preference| {
            self.source.as_ref() == Some(&preference.region.source)
                && artifact
                    .readability
                    .iter()
                    .any(|decision| decision.region == preference.region)
        });
        if self.config.regions.len() != previous_count {
            self.notice = Some(
                "Stale or unavailable region choices were cleared. Current region choices remain available.",
            );
        }
        if self.source.as_ref() != Some(&readability_source(&artifact.input, &artifact.config)) {
            self.notice = Some(
                "Region choices from this artifact version are cleared when reanalyzing. Select regions from the new result.",
            );
        }
        self.seen_selection = self.region.as_ref().and(state.selected.clone());
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        artifact: &Artifact,
        state: &ViewState,
        busy: bool,
    ) -> Option<DecompileRequest> {
        let mut apply = false;
        framework::section(ui, "Reading choices", |ui| {
            global_controls(ui, &mut self.config);
            if let Some(notice) = self.notice {
                ui.colored_label(palette::WARNING, notice);
            }
            self.follow_selection(artifact, state);
            if artifact.readability.is_empty() {
                ui.label("No region choices were retained for this result.");
            } else {
                let label = self
                    .region
                    .as_ref()
                    .map(|region| region_label(artifact, region))
                    .unwrap_or_else(|| "Choose a source region".into());
                let combo = egui::ComboBox::from_id_salt("reading_region")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for decision in &artifact.readability {
                            controls::tab_value(
                                ui,
                                &mut self.region,
                                Some(decision.region.clone()),
                                region_label(artifact, &decision.region),
                                Density::Compact,
                            );
                        }
                    });
                controls::observe(ui, &combo.response, "Source region", "combobox", None);
                if let Some(decision) = self.region.as_ref().and_then(|region| {
                    artifact
                        .readability
                        .iter()
                        .find(|decision| &decision.region == region)
                }) {
                    let mut preference = self
                        .config
                        .regions
                        .iter()
                        .find(|preference| preference.region == decision.region)
                        .map_or(StructurePreference::Auto, |preference| preference.structure);
                    let before = preference;
                    ui.horizontal_wrapped(|ui| {
                        for (value, label) in [
                            (StructurePreference::Auto, "Auto"),
                            (StructurePreference::Natural, "Natural"),
                            (StructurePreference::Dispatcher, "Dispatcher"),
                        ] {
                            controls::tab_value(
                                ui,
                                &mut preference,
                                value,
                                label,
                                Density::Compact,
                            );
                        }
                    });
                    if preference != before {
                        self.set_region(decision.region.clone(), preference);
                    }
                    decision_details(ui, decision);
                }
            }
            apply = controls::action_enabled(
                ui,
                !busy && self.config != artifact.config.readability,
                "Apply reading choices",
                Density::Regular,
            )
            .on_hover_text("Reanalyze the same bytecode with these reading preferences")
            .clicked();
            if self.config != artifact.config.readability {
                ui.label("Pending choices; the current result is still shown.");
            }
        });
        apply.then(|| self.request(artifact))
    }

    fn set_region(&mut self, region: RegionAnchor, structure: StructurePreference) {
        self.config
            .regions
            .retain(|preference| preference.region != region);
        if structure != StructurePreference::Auto {
            self.config
                .regions
                .push(RegionPreference { region, structure });
        }
    }

    fn follow_selection(&mut self, artifact: &Artifact, state: &ViewState) {
        if self.seen_selection == state.selected {
            return;
        }
        self.seen_selection = state.selected.clone();
        let Some(selected) = state.selected.as_ref() else {
            return;
        };
        let Some(stage) = artifact.stage(selected.at.stage) else {
            return;
        };
        let entry = state
            .index
            .stage(stage.id)
            .containing_block(selected.at.entity)
            .and_then(|index| stage.ir.blocks.get(index))
            .map(|block| block.id);
        if let Some(decision) = artifact
            .readability
            .iter()
            .find(|decision| Some(decision.region.entry) == entry)
        {
            self.region = Some(decision.region.clone());
        }
    }

    fn request(&self, artifact: &Artifact) -> DecompileRequest {
        let source = readability_source(&artifact.input, &artifact.config);
        let mut readability = self.config.clone();
        readability
            .regions
            .retain(|preference| preference.region.source == source);
        DecompileRequest {
            bytecode: artifact
                .input
                .runtime_bytecode
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            fork: match artifact.input.fork {
                evm_abstract_protocol::decompile_model::Fork::Cancun => Fork::Cancun,
                evm_abstract_protocol::decompile_model::Fork::Prague => Fork::Prague,
                evm_abstract_protocol::decompile_model::Fork::Osaka => Fork::Osaka,
            },
            strategy: artifact.config.strategy,
            execution_domain: artifact.config.execution_domain,
            limits: artifact.config.limits.clone(),
            readability,
        }
    }

    pub fn status(&self) -> String {
        format!(
            "Reading profile draft: {:?}; Procedures draft: {:?}; Region choices: {}{}",
            self.config.profile,
            self.config.procedures,
            self.config.regions.len(),
            self.notice
                .map_or(String::new(), |notice| format!("; {notice}"))
        )
    }
}

pub(super) fn global_controls(ui: &mut egui::Ui, config: &mut ReadabilityConfig) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Reading style");
        for (value, label) in [
            (ReadabilityProfile::Balanced, "Balanced"),
            (ReadabilityProfile::Linear, "Linear"),
            (ReadabilityProfile::Compact, "Compact"),
        ] {
            controls::tab_value(ui, &mut config.profile, value, label, Density::Compact);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Procedures");
        for (value, label) in [
            (ProcedurePreference::Auto, "Auto procedures"),
            (ProcedurePreference::Inline, "Inline"),
            (ProcedurePreference::Shared, "Shared"),
        ] {
            controls::tab_value(ui, &mut config.procedures, value, label, Density::Compact);
        }
    });
}

fn source_label(artifact: &Artifact, id: EntityId) -> String {
    artifact
        .stages
        .iter()
        .rev()
        .find_map(|stage| stage.ir.block(id))
        .and_then(|block| block.source)
        .map_or_else(
            || format!("source b{}", id.0),
            |source| format!("0x{:04x} · b{}", source.pc, id.0),
        )
}

fn region_label(artifact: &Artifact, region: &RegionAnchor) -> String {
    let mut label = source_label(artifact, region.entry);
    if let Some(continuation) = region.continuation {
        label.push_str(&format!(" → {}", source_label(artifact, continuation)));
    }
    if let Some(header) = region.loop_header {
        label.push_str(&format!(" · loop {}", source_label(artifact, header)));
    }
    label
}

fn decision_details(ui: &mut egui::Ui, decision: &RegionDecision) {
    let selected = match decision.selected {
        StructureForm::Natural => "Natural",
        StructureForm::Dispatcher => "Dispatcher",
    };
    let reason = match decision.reason {
        ReadabilityReason::UserPreference => "Selected by the requested preference",
        ReadabilityReason::NaturalControl => "Natural control structure retained",
        ReadabilityReason::LowerReadingCost => "Lower measured presentation cost",
        ReadabilityReason::MultiEntryControl => "Multiple-entry control requires explicit dispatch",
        ReadabilityReason::Expansion => "Avoided repeated expansion of shared control",
        ReadabilityReason::PlanningFallback => "Retained the available verified plan",
        ReadabilityReason::DepthFallback => "Avoided excessive structure depth",
        ReadabilityReason::PreferenceFallback => {
            "Requested form unavailable; retained a verified alternative"
        }
    };
    let text = format!("Selected: {selected}. {reason}.");
    let response = ui.label(&text);
    controls::observe(ui, &response, &text, "decompile-reading-choice", None);
    metrics(ui, "Natural", decision.natural);
    metrics(ui, "Dispatcher", decision.dispatcher);
    ui.label("Candidate structure counts");
}

fn metrics(ui: &mut egui::Ui, name: &str, metrics: Option<ReadabilityMetrics>) {
    let Some(metrics) = metrics else {
        ui.label(format!("{name}: candidate not measured"));
        return;
    };
    let text = format!(
        "{name}: {} source operations · {} repeated · {} conditions · estimated nesting {} · {} entry values · {} dispatch blocks / {} transitions · {} syntax nodes",
        metrics.operations,
        metrics.duplicated_operations,
        metrics.conditions,
        metrics.max_nesting,
        metrics.entry_values,
        metrics.dispatcher_blocks,
        metrics.dispatcher_transitions,
        metrics.syntax_nodes,
    );
    let response = ui.label(&text);
    controls::observe(ui, &response, &text, "decompile-reading-metrics", None);
}
