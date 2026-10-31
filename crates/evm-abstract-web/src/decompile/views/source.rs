use super::super::{model::ViewState, rows};
use crate::{
    controls::{self, Density},
    framework,
};
use evm_abstract_protocol::decompile_model::{Artifact, ArtifactId, StageKind};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Source {
    artifact: Option<ArtifactId>,
    positions: Vec<(usize, usize)>,
    pcs: BTreeMap<usize, usize>,
    pc_input: String,
}

impl Source {
    pub fn show(&mut self, ui: &mut egui::Ui, artifact: &Artifact, state: &mut ViewState) {
        ui.strong("Original bytecode and all descendants");
        let Some(stage) = artifact
            .stages
            .iter()
            .find(|stage| stage.kind == StageKind::Decoded)
        else {
            ui.label("No decoded source was published for this attempt");
            return;
        };
        if self.artifact.as_ref() != Some(&artifact.id) {
            self.artifact = Some(artifact.id.clone());
            self.positions.clear();
            self.pcs.clear();
            self.pc_input.clear();
            for (block, body) in stage.ir.blocks.iter().enumerate() {
                for (operation, op) in body.operations.iter().enumerate() {
                    if let Some(span) = op.source {
                        self.pcs.insert(span.pc, self.positions.len());
                        self.positions.push((block, operation));
                    }
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Original PC (hex)");
            framework::scoped_editor(ui, "Original PC", &mut self.pc_input, "0x0000", 100.0);
            let digits = self.pc_input.strip_prefix("0x").unwrap_or(&self.pc_input);
            let pc = usize::from_str_radix(digits, 16)
                .ok()
                .filter(|pc| self.pcs.contains_key(pc));
            if controls::action_enabled(ui, pc.is_some(), "Go to original PC", Density::Regular)
                .clicked()
            {
                state.select_source(artifact, pc.expect("known original instruction"));
                state.source_navigation = true;
            }
        });
        let mut scroll = egui::ScrollArea::both().id_salt("decompile_original");
        if state.source_navigation
            && let Some(position) = state.source_pc.and_then(|pc| self.pcs.get(&pc))
        {
            scroll = scroll.vertical_scroll_offset(rows::offset(ui, *position));
        }
        scroll.show_rows(ui, rows::HEIGHT, self.positions.len(), |ui, range| {
            for position in range {
                let (block, operation) = self.positions[position];
                let op = &stage.ir.blocks[block].operations[operation];
                let span = op.source.expect("indexed source span");
                let bytes = artifact
                    .input
                    .runtime_bytecode
                    .get(span.pc..span.end_pc)
                    .unwrap_or_default()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let label = format!("Source 0x{:04x}: {} {bytes}", span.pc, op.name);
                rows::row(ui, position, |ui| {
                    let response = controls::tab(
                        ui,
                        state.source_pc == Some(span.pc),
                        &label,
                        Density::Regular,
                    );
                    controls::observe(
                        ui,
                        &response,
                        &label,
                        "decompile-source",
                        Some(state.source_pc == Some(span.pc)),
                    );
                    if response.clicked() {
                        state.select_source(artifact, span.pc);
                    }
                    if state.source_pc == Some(span.pc) && rows::visible(ui, &response) {
                        state.source_navigation = false;
                    }
                });
            }
        });
    }
}
