//! Cached measured graph scenes; numeric layout keys stay separate from IR IDs.
use super::{
    super::{diff::Differences, model::ViewState, structured},
    stage_label,
};
use crate::{graph_view, palette};
use egui::Ui;
use evm_abstract_protocol::decompile_model::{Artifact, EdgeKind, EntityRef, Stage, StageKind};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Graph {
    canvas: graph_view::Canvas,
    identity: Option<egui::Id>,
    content: graph_view::Scene,
}
impl Graph {
    pub fn prepare_input(&mut self, input: &mut egui::RawInput) {
        self.canvas.prepare_input(input);
    }
    pub fn show(
        &mut self,
        ui: &mut Ui,
        artifact: &Artifact,
        stage: &Stage,
        state: &mut ViewState,
        differences: &Differences,
        before: bool,
    ) {
        let title = if before { "Before CFG" } else { "Current CFG" };
        let index = state.index.stage(stage.id);
        let identity = egui::Id::new((
            &artifact.id.0,
            stage.id.0,
            state.before.map(|id| id.0),
            state.current.map(|id| id.0),
            title,
        ));
        if self.identity != Some(identity) {
            self.identity = Some(identity);
            self.content = build(
                stage,
                &index,
                differences,
                before,
                &artifact.input.runtime_bytecode,
            );
        }
        let selected = state
            .selected_in(artifact, stage.id)
            .and_then(|id| index.containing_block(id));
        if state.entity_navigation && selected.is_some() {
            self.canvas.focus_selected();
            state.entity_navigation = false;
        }
        ui.push_id(title, |ui| {
            ui.label(stage_label(stage));
            if let Some(key) = self
                .canvas
                .show(ui, identity, title, &self.content, selected)
                && let Some(block) = stage.ir.blocks.get(key)
            {
                state.select(
                    artifact,
                    EntityRef {
                        stage: stage.id,
                        entity: block.id,
                    },
                );
            }
        });
    }
}

pub(super) fn build(
    stage: &Stage,
    index: &super::super::index::StageIndex,
    differences: &Differences,
    before: bool,
    bytecode: &[u8],
) -> graph_view::Scene {
    let positions: BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id, index))
        .collect();
    graph_view::Scene {
        cards: stage
            .ir
            .blocks
            .iter()
            .enumerate()
            .map(|(key, block)| graph_view::Card {
                key,
                title: format!(
                    "b{} · {}",
                    block.id.0,
                    block.source.map_or_else(
                        || "generated".into(),
                        |span| format!("source 0x{:04x}", span.pc)
                    )
                ),
                lines: block
                    .operations
                    .iter()
                    .take(graph_view::PREVIEW_LINES)
                    .map(|op| {
                        if stage.kind == StageKind::Decoded {
                            structured::decoded_operation(op, bytecode)
                        } else {
                            structured::operation(op, &stage.ir, index)
                        }
                    })
                    .chain(std::iter::once_with(|| {
                        structured::terminator(&block.terminator)
                    }))
                    .take(graph_view::PREVIEW_LINES)
                    .collect(),
                total_lines: block.operations.len() + 1,
                markers: differences.markers(block.id, before),
                boundary: block.terminator.is_partial(),
            })
            .collect(),
        connections: stage
            .ir
            .edges
            .iter()
            .enumerate()
            .filter_map(|(key, edge)| {
                let markers = differences.markers(edge.id, before);
                Some(graph_view::Connection {
                    key,
                    from: *positions.get(&edge.from)?,
                    to: *positions.get(&edge.to)?,
                    label: format!(
                        "e{} {:?}{}",
                        edge.id.0,
                        edge.kind,
                        markers
                            .iter()
                            .map(|(label, _)| format!(" · {label}"))
                            .collect::<String>()
                    ),
                    kind: match edge.kind {
                        EdgeKind::Fallthrough => evm_abstract_layout::EdgeKind::Fallthrough,
                        EdgeKind::Jump => evm_abstract_layout::EdgeKind::Jump,
                        EdgeKind::True => evm_abstract_layout::EdgeKind::BranchTrue,
                        EdgeKind::False => evm_abstract_layout::EdgeKind::BranchFalse,
                    },
                    color: markers.first().map_or(palette::BLUE, |(_, color)| *color),
                })
            })
            .collect(),
    }
}
