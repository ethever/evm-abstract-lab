//! Virtual rows retain every IR entity while formatting only the visible range.

use super::{
    super::{diff::Differences, model::ViewState, structured},
    entity_line, stage_label,
};
use egui::Ui;
use evm_abstract_protocol::decompile_model::{Artifact, EntityId, EntityRef, Stage};

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
        ui.label(format!(
            "Facts: {:?} · original bytes: {}",
            stage.ir.fact_scope, stage.ir.bytecode_len
        ));
        let mut scroll = egui::ScrollArea::both().id_salt(identity);
        if state.entity_navigation
            && let Some(selected) = state.selected_in(artifact, stage.id)
            && let Some(position) = self
                .rows
                .iter()
                .position(|row| row.entity(stage) == Some(selected))
        {
            scroll = scroll.vertical_scroll_offset(position as f32 * 22.0);
            state.entity_navigation = false;
        }
        scroll.show_rows(ui, 22.0, self.rows.len(), |ui, range| {
            for position in range {
                ui.push_id(position, |ui| {
                    let row = &self.rows[position];
                    let text = row.text(stage);
                    if let Some(entity) = row.entity(stage) {
                        entity_line(
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
    fn text(&self, stage: &Stage) -> String {
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
                format!("  o{}: {}", op.id.0, structured::operation(op, &stage.ir))
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
