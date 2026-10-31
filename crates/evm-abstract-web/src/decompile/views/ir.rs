//! Virtual rows retain every IR entity while formatting only the visible range.

use super::{
    super::{diff::Differences, index::StageIndex, model::ViewState, rows, structured},
    entity_line, stage_label,
};
use egui::Ui;
use evm_abstract_protocol::decompile_model::{Artifact, EntityId, EntityRef, Stage, StageKind};

enum Row {
    Block(usize),
    Operation(usize, usize),
    Terminator(usize),
    Edge(usize),
    Value(usize),
    Heading(&'static str),
}

#[derive(Default)]
pub(super) struct Ir {
    identity: Option<egui::Id>,
    rows: Vec<Row>,
}

impl Ir {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        artifact: &Artifact,
        stage: &Stage,
        state: &mut ViewState,
        differences: &Differences,
        before: bool,
    ) {
        let title = if before { "Before IR" } else { "Current IR" };
        let index = state.index.stage(stage.id);
        let identity = egui::Id::new((&artifact.id.0, stage.id.0, title));
        if self.identity != Some(identity) {
            self.identity = Some(identity);
            self.rows.clear();
            for (block, body) in stage.ir.blocks.iter().enumerate() {
                self.rows.push(Row::Block(block));
                for operation in 0..body.operations.len() {
                    self.rows.push(Row::Operation(block, operation));
                }
                self.rows.push(Row::Terminator(block));
            }
            self.rows
                .push(Row::Heading("Edges and simultaneous arguments"));
            self.rows.extend((0..stage.ir.edges.len()).map(Row::Edge));
            self.rows
                .push(Row::Heading("Value definitions and local facts"));
            self.rows.extend((0..stage.ir.values.len()).map(Row::Value));
        }
        ui.strong(format!("{title} · {}", stage_label(stage)));
        if stage.kind == StageKind::Decoded {
            ui.label(
                "Original disassembly and unrecovered placeholders; no recovered SSA in this stage",
            );
        }
        if stage.kind == StageKind::Decoded {
            ui.label(format!("Original bytes: {}", stage.ir.bytecode_len));
        } else {
            ui.label(format!(
                "Facts: {:?} · original bytes: {}",
                stage.ir.fact_scope, stage.ir.bytecode_len
            ));
        }
        let mut scroll = egui::ScrollArea::both().id_salt(identity);
        if !before
            && state.entity_navigation
            && let Some(selected) = state.selected_in(artifact, stage.id)
            && let Some(position) = self
                .rows
                .iter()
                .position(|row| row.entity(stage) == Some(selected))
        {
            scroll = scroll.vertical_scroll_offset(rows::offset(ui, position));
        }
        scroll.show_rows(ui, rows::HEIGHT, self.rows.len(), |ui, range| {
            for position in range {
                rows::row(ui, position, |ui| {
                    let row = &self.rows[position];
                    let text = row.text(stage, &index, &artifact.input.runtime_bytecode);
                    if let Some(entity) = row.entity(stage) {
                        let visible = entity_line(
                            ui,
                            artifact,
                            state,
                            EntityRef {
                                stage: stage.id,
                                entity,
                            },
                            &text,
                            "decompile-ir",
                            &differences.markers(entity, before),
                        );
                        if !before
                            && visible
                            && state.selected_in(artifact, stage.id) == Some(entity)
                        {
                            state.entity_navigation = false;
                        }
                    } else {
                        ui.strong(text);
                    }
                });
            }
        });
    }
}

impl Row {
    fn entity(&self, stage: &Stage) -> Option<EntityId> {
        match *self {
            Self::Block(index) | Self::Terminator(index) => Some(stage.ir.blocks[index].id),
            Self::Operation(block, operation) => {
                Some(stage.ir.blocks[block].operations[operation].id)
            }
            Self::Edge(index) => Some(stage.ir.edges[index].id),
            Self::Value(index) => Some(stage.ir.values[index].id),
            Self::Heading(_) => None,
        }
    }
    fn text(&self, stage: &Stage, index: &StageIndex, bytecode: &[u8]) -> String {
        if stage.kind == StageKind::Decoded {
            return match *self {
                Self::Operation(block, operation) => structured::decoded_operation(
                    &stage.ir.blocks[block].operations[operation],
                    bytecode,
                ),
                Self::Block(position) => format!(
                    "Decoded block b{} · recovery unavailable",
                    stage.ir.blocks[position].id.0
                ),
                Self::Value(position) => format!(
                    "Unrecovered placeholder e{} · {:?}",
                    stage.ir.values[position].id.0, stage.ir.values[position].fact
                ),
                Self::Terminator(_) => {
                    "Recovery pending; decoded instructions do not establish a continuation".into()
                }
                Self::Heading(_) => "Original decode metadata (not recovered SSA)".into(),
                Self::Edge(position) => format!(
                    "Decoded edge e{} · not a recovered SSA transfer",
                    stage.ir.edges[position].id.0
                ),
            };
        }
        match *self {
            Self::Block(index) => {
                let block = &stage.ir.blocks[index];
                format!(
                    "b{}({}) [memory m{} · effect q{}]",
                    block.id.0,
                    structured::values(&block.parameters),
                    block.memory_parameter.0,
                    block.effect_parameter.0
                )
            }
            Self::Operation(block, operation) => {
                let op = &stage.ir.blocks[block].operations[operation];
                format!(
                    "  o{}: {}",
                    op.id.0,
                    structured::operation(op, &stage.ir, index)
                )
            }
            Self::Terminator(index) => format!(
                "  {}",
                structured::terminator(&stage.ir.blocks[index].terminator)
            ),
            Self::Edge(index) => {
                let edge = &stage.ir.edges[index];
                format!(
                    "e{}: b{} → b{} {:?} ({}) [memory m{} · effect q{}]",
                    edge.id.0,
                    edge.from.0,
                    edge.to.0,
                    edge.kind,
                    structured::values(&edge.arguments),
                    edge.memory.0,
                    edge.effect.0
                )
            }
            Self::Value(index) => {
                let value = &stage.ir.values[index];
                format!("v{}: {:?} · {:?}", value.id.0, value.kind, value.fact)
            }
            Self::Heading(text) => text.into(),
        }
    }
}
