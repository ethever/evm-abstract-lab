//! Virtual source rows use content-sized PC and opcode/operand columns.

use evm_abstract_notation::Symbol;
mod tooltip;

use egui::{FontId, ScrollArea, Sense, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, DisasmInstruction};

use super::{ROW_HEIGHT, heading, text};
use crate::{app::Selection, palette};

pub(crate) fn disassembly(
    ui: &mut Ui,
    report: &AnalysisReport,
    program: Option<usize>,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    // Exactly one instance is visible at a time, whether standalone or inside
    // responsive tiles. Its absolute UI identity preserves both scroll axes
    // when the parent tree changes, without overriding manual navigation.
    ui.scope_builder(
        egui::UiBuilder::new().id(egui::Id::new("disassembly_view")),
        |ui| contents(ui, report, program, selection, previous),
    );
}

fn contents(
    ui: &mut Ui,
    report: &AnalysisReport,
    program: Option<usize>,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    let source = program.and_then(|id| report.programs.iter().find(|source| source.id == id));
    heading(
        ui,
        "DISASSEMBLY",
        &source.map_or_else(
            || "No captured bytecode".into(),
            |source| {
                format!(
                    "{} · {:?} · {} bytes",
                    Symbol::Program(source.id),
                    source.kind,
                    source.bytecode.trim_start_matches("0x").len() / 2
                )
            },
        ),
    );
    if source.is_none() {
        super::empty(ui, "No captured bytecode for this frame or account");
        return;
    }
    let rows = rows(report, program, *selection);
    let focus = (*selection != *previous)
        .then(|| rows.iter().position(|row| row.is_focused(*selection)))
        .flatten();
    *previous = *selection;
    let widths = column_widths(ui, &rows);
    ScrollArea::horizontal()
        .id_salt("disasm_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
            ui.set_min_width(widths.iter().sum::<f32>() + 6.0);
            let height = ui.available_height() - ROW_HEIGHT;
            let mut table = TableBuilder::new(ui)
                .id_salt("disasm_rows")
                .column(Column::exact(widths[0]))
                .column(Column::remainder().at_least(widths[1]))
                .sense(Sense::click())
                .striped(false)
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_scroll_height(height.max(0.0));
            if let Some(index) = focus {
                table = table.scroll_to_row(index, Some(egui::Align::Center));
            }
            table
                .header(ROW_HEIGHT, |mut header| {
                    for label in ["PC", "OPCODE / OPERAND"] {
                        header.col(|ui| {
                            text(
                                ui,
                                ui.max_rect().left_center() + Vec2::new(4.0, 0.0),
                                label,
                                palette::MUTED,
                                10.0,
                            );
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_HEIGHT, rows.len(), |mut table_row| {
                        let row = &rows[table_row.index()];
                        table_row.set_selected(row.is_selected(*selection));
                        table_row.set_overline(matches!(row, Row::Block { .. }));
                        table_row.col(|ui| {
                            if let Row::Instruction { instruction, .. } = row {
                                text(
                                    ui,
                                    ui.max_rect().left_center() + Vec2::new(4.0, 0.0),
                                    format!("{:04x}", instruction.pc),
                                    palette::MUTED,
                                    12.0,
                                );
                            }
                        });
                        table_row.col(|ui| {
                            let origin = ui.max_rect().left_center() + Vec2::new(4.0, 0.0);
                            match row {
                                Row::Block {
                                    id, pc, selected, ..
                                } => {
                                    text(
                                        ui,
                                        origin,
                                        format!(
                                            "{notation_0}  ·  0x{pc:04x}",
                                            notation_0 = Symbol::Block(*id)
                                        ),
                                        if *selected {
                                            palette::ACCENT
                                        } else {
                                            palette::MUTED
                                        },
                                        12.0,
                                    );
                                }
                                Row::Instruction {
                                    instruction,
                                    executed,
                                    ..
                                } => {
                                    let color = if !instruction.valid {
                                        palette::ERROR
                                    } else if *executed {
                                        palette::TEXT
                                    } else {
                                        palette::MUTED
                                    };
                                    let rect = text(ui, origin, &instruction.name, color, 12.0);
                                    if let Some(immediate) = &instruction.immediate {
                                        text(
                                            ui,
                                            egui::pos2(rect.right(), origin.y),
                                            format!(" {immediate}"),
                                            palette::BLUE,
                                            12.0,
                                        );
                                    }
                                }
                            }
                        });
                        let response = table_row.response();
                        if response.clicked() {
                            *selection = row.target();
                        }
                        if let Row::Instruction {
                            instruction,
                            executed,
                            ..
                        } = row
                        {
                            response.on_hover_ui(|ui| {
                                ui.label(crate::notation::widget(
                                    ui,
                                    tooltip::instruction(instruction, *executed),
                                ));
                            });
                        }
                    });
                });
        });
}

/// Desired outer pane width uses the same columns and selected-frame source as
/// the table itself. A bounded parent can still expose overflow by scrolling.
pub(crate) fn natural_width(
    ui: &Ui,
    report: &AnalysisReport,
    program: Option<usize>,
    selection: Selection,
) -> f32 {
    column_widths(ui, &rows(report, program, selection))
        .iter()
        .sum::<f32>()
        + 6.0
        + ui.spacing().scroll.allocated_width()
}

fn column_widths(ui: &Ui, rows: &[Row<'_>]) -> [f32; 2] {
    let glyph = ui
        .painter()
        .layout_no_wrap("0".into(), FontId::monospace(12.0), palette::TEXT)
        .size()
        .x;
    content_columns(rows).map(|columns| columns as f32 * glyph + 8.0)
}

enum Row<'a> {
    Block {
        id: usize,
        pc: usize,
        state: Option<usize>,
        selected: bool,
    },
    Instruction {
        instruction: &'a DisasmInstruction,
        state: Option<usize>,
        selected_block: bool,
        executed: bool,
    },
}

impl Row<'_> {
    fn target(&self) -> Selection {
        match self {
            Self::Block { state, pc, .. } => Selection {
                state: *state,
                pc: Some(*pc),
            },
            Self::Instruction {
                instruction, state, ..
            } => Selection {
                state: *state,
                pc: Some(instruction.pc),
            },
        }
    }

    fn is_selected(&self, selection: Selection) -> bool {
        match self {
            Self::Block { selected, .. } => *selected,
            Self::Instruction {
                instruction,
                state,
                selected_block,
                ..
            } => {
                selection.pc == Some(instruction.pc)
                    && (*selected_block || (selection.state.is_none() && state.is_none()))
            }
        }
    }

    fn is_focused(&self, selection: Selection) -> bool {
        match self {
            Self::Block { selected, .. } => *selected && selection.pc.is_none(),
            Self::Instruction { .. } => self.is_selected(selection),
        }
    }
}

fn rows(report: &AnalysisReport, program: Option<usize>, selection: Selection) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let Some(source) = program.and_then(|id| report.programs.iter().find(|source| source.id == id))
    else {
        return rows;
    };
    for block in &source.blocks {
        let selected_state = report.cfg.iter().find(|state| {
            state.program == program
                && state.basic_block == block.id
                && Some(state.id) == selection.state
        });
        let state = selected_state.or_else(|| {
            report
                .cfg
                .iter()
                .find(|state| state.program == program && state.basic_block == block.id)
        });
        rows.push(Row::Block {
            id: block.id,
            pc: block.start_pc,
            state: state.map(|state| state.id),
            selected: selected_state.is_some(),
        });
        rows.extend(
            block
                .instructions
                .iter()
                .map(|instruction| Row::Instruction {
                    instruction,
                    state: state.map(|state| state.id),
                    selected_block: selected_state.is_some(),
                    executed: state.is_some_and(|state| {
                        super::coverage(report, state.id) == BlockCoverage::Current
                            && state.executed_pcs.contains(&instruction.pc)
                    }),
                }),
        );
    }
    rows
}

fn content_columns(rows: &[Row<'_>]) -> [usize; 2] {
    rows.iter().fold([4, 15], |mut columns, row| {
        match row {
            Row::Block { id, pc, .. } => {
                columns[1] = columns[1].max(
                    format!(
                        "{notation_0}  ·  0x{pc:04x}",
                        notation_0 = Symbol::Block(*id)
                    )
                    .chars()
                    .count(),
                )
            }
            Row::Instruction { instruction, .. } => {
                columns[0] = columns[0].max(format!("{:04x}", instruction.pc).len());
                columns[1] = columns[1].max(
                    instruction.name.chars().count()
                        + instruction
                            .immediate
                            .as_ref()
                            .map_or(0, |value| value.chars().count() + 1),
                );
            }
        }
        columns
    })
}

#[cfg(test)]
mod tests;
