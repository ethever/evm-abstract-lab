//! SSA uses compact virtual table rows. Tokens retain semantic coloring while
//! native state/PC targets keep table selection linked to the graph and source.

use egui::{Color32, FontId, ScrollArea, Sense, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, InstructionProgress};

use super::{ROW_HEIGHT, heading, row_background, text};
use crate::{app::Selection, palette};

pub(crate) fn ssa(
    ui: &mut Ui,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    // Exactly one instance is visible at a time, whether standalone or inside
    // responsive tiles. Its absolute UI identity preserves both scroll axes
    // when the parent tree changes, without overriding manual navigation.
    ui.scope_builder(egui::UiBuilder::new().id(egui::Id::new("ssa_view")), |ui| {
        contents(ui, report, selection, previous)
    });
}

fn contents(
    ui: &mut Ui,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    heading(
        ui,
        "SSA",
        &format!(
            "{} · {} values · {} effects",
            if report.ssa.complete {
                "verified complete"
            } else {
                "partial coverage"
            },
            report.ssa.value_count,
            report.ssa.effect_count,
        ),
    );
    let rows = rows(report);
    let focus = (*selection != *previous)
        .then(|| {
            rows.iter().position(|row| {
                row.target == Some(*selection) && (selection.pc.is_some() || row.header)
            })
        })
        .flatten();
    *previous = *selection;
    // Width comes from the actual tokens, including effects, exit stacks and
    // transitions. A short report therefore fits a narrow pane without a
    // permanent blank gutter; long operands remain horizontally reachable.
    let width = row_width(ui, &rows);
    ScrollArea::horizontal()
        .id_salt("ssa_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_min_width(width);
            let height = ui.available_height();
            let mut table = TableBuilder::new(ui)
                .id_salt("ssa_rows")
                .column(Column::remainder().at_least(width))
                .sense(Sense::click())
                .striped(false)
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .max_scroll_height(height);
            if let Some(index) = focus {
                // TableBuilder scrolls only vertically; selection must never
                // shift the independently scrolled leading columns sideways.
                table = table.scroll_to_row(index, Some(egui::Align::Center));
            }
            table.body(|body| {
                body.rows(ROW_HEIGHT, rows.len(), |mut table_row| {
                    let row = &rows[table_row.index()];
                    let selected = row.target.is_some_and(|target| {
                        target.state == selection.state
                            && (row.header || (target.pc.is_some() && target.pc == selection.pc))
                    });
                    table_row.col(|ui| {
                        let rect = ui.max_rect();
                        if row.header {
                            ui.painter().rect_filled(rect, 0.0, palette::PANEL);
                        }
                        row_background(ui, rect, selected, ui.rect_contains_pointer(rect));
                        let mut cursor = rect.left_center() + Vec2::new(4.0, 0.0);
                        for token in &row.tokens {
                            cursor.x = text(ui, cursor, &token.text, token.color, 12.0).right();
                        }
                    });
                    let response = table_row.response();
                    if response.clicked()
                        && let Some(target) = row.target
                    {
                        *selection = target;
                    }
                    response.on_hover_text(&row.tooltip);
                });
            });
        });
}

/// SSA displays the complete report, so pane sizing measures every retained
/// block, including phi inputs, effects and transition rows.
pub(crate) fn natural_width(ui: &Ui, report: &AnalysisReport) -> f32 {
    row_width(ui, &rows(report)) + ui.spacing().scroll.allocated_width()
}

fn row_width(ui: &Ui, rows: &[Row]) -> f32 {
    let glyph_width = ui
        .painter()
        .layout_no_wrap("0".into(), FontId::monospace(12.0), palette::TEXT)
        .size()
        .x;
    rows.iter().map(Row::columns).max().unwrap_or_default() as f32 * glyph_width + 12.0
}

struct Token {
    text: String,
    color: Color32,
}

struct Row {
    tokens: Vec<Token>,
    target: Option<Selection>,
    header: bool,
    tooltip: String,
}

impl Row {
    fn new() -> Self {
        Self {
            tokens: Vec::new(),
            target: None,
            header: false,
            tooltip: String::new(),
        }
    }

    fn token(&mut self, text: impl Into<String>, color: Color32) {
        self.tokens.push(Token {
            text: text.into(),
            color,
        });
    }

    fn columns(&self) -> usize {
        self.tokens
            .iter()
            .map(|token| token.text.chars().count())
            .sum()
    }

    fn plain_text(&self) -> String {
        self.tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect()
    }
}

fn values(values: &[usize]) -> String {
    values
        .iter()
        .map(|id| format!("%{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn frames(frames: &[Vec<usize>]) -> String {
    frames
        .iter()
        .enumerate()
        .map(|(frame, stack)| format!("f{frame} [{}]", values(stack)))
        .collect::<Vec<_>>()
        .join("  ")
}

fn rows(report: &AnalysisReport) -> Vec<Row> {
    let mut rows = Vec::new();
    for block in &report.ssa.blocks {
        let mut header = Row::new();
        header.header = true;
        header.target = Some(Selection {
            state: Some(block.state),
            pc: None,
        });
        header.token(format!("S{}", block.state), palette::ACCENT);
        header.token(
            format!(
                "  {:?} · {} incoming",
                block.coverage,
                if block.incoming_complete {
                    "complete"
                } else {
                    "open"
                }
            ),
            if block.coverage == BlockCoverage::Current {
                palette::MUTED
            } else {
                palette::WARNING
            },
        );
        header.tooltip = header.plain_text();
        rows.push(header);
        for phi in &block.phis {
            let mut row = Row::new();
            row.token(format!("%{}", phi.result), palette::PURPLE);
            row.token(" = φ(", palette::MUTED);
            for (index, input) in phi.inputs.iter().enumerate() {
                if index > 0 {
                    row.token(", ", palette::MUTED);
                }
                row.token(
                    format!("S{}:%{}", input.predecessor, input.value),
                    palette::BLUE,
                );
            }
            row.token(")", palette::MUTED);
            row.token(format!("  f{}/slot{}", phi.frame, phi.slot), palette::MUTED);
            row.tooltip = format!(
                "{}\nFrame {}, stack slot {} (bottom to top)\n{}",
                row.plain_text(),
                phi.frame,
                phi.slot,
                phi.inputs
                    .iter()
                    .map(|input| format!(
                        "edge e{} from S{}: %{}",
                        input.edge, input.predecessor, input.value
                    ))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            rows.push(row);
        }
        let mut effect = Row::new();
        effect.token(
            format!(
                "μ{} = effect φ({}){}",
                block.effect,
                block
                    .effect_inputs
                    .iter()
                    .map(|input| format!("e{}:μ{}", input.edge, input.effect))
                    .collect::<Vec<_>>()
                    .join(", "),
                if block.open_incoming.is_empty() {
                    String::new()
                } else {
                    format!(" · open {:?}", block.open_incoming)
                },
            ),
            palette::MUTED,
        );
        effect.tooltip = effect.plain_text();
        rows.push(effect);
        for instruction in &block.instructions {
            let mut row = Row::new();
            row.target = Some(Selection {
                state: Some(block.state),
                pc: Some(instruction.pc),
            });
            row.token(format!("{:04x}  ", instruction.pc), palette::MUTED);
            for (index, result) in instruction.results.iter().enumerate() {
                if index > 0 {
                    row.token(", ", palette::MUTED);
                }
                row.token(format!("%{result}"), palette::PURPLE);
            }
            if !instruction.results.is_empty() {
                row.token(" = ", palette::MUTED);
            }
            row.token(
                &instruction.name,
                if instruction.fault {
                    palette::ERROR
                } else {
                    palette::TEXT
                },
            );
            if let Some(value) = &instruction.immediate {
                row.token(format!(" {value}"), palette::BLUE);
            }
            for operand in &instruction.operands {
                row.token(format!(" %{operand}"), palette::BLUE);
            }
            row.token(
                format!(
                    " · μ{}→{}",
                    instruction.effect_input,
                    instruction
                        .effect_result
                        .map_or_else(|| "pending".into(), |id| format!("μ{id}"))
                ),
                if instruction.progress == InstructionProgress::Completed {
                    palette::MUTED
                } else {
                    palette::WARNING
                },
            );
            // Ordinary completion is implicit in the compact line; every
            // partial or fault phase stays explicit, with the exact phase also
            // recorded in the full row tooltip.
            if instruction.progress != InstructionProgress::Completed {
                row.token(format!(" · {:?}", instruction.progress), palette::WARNING);
            }
            row.tooltip = format!(
                "{}\nState S{} · pc 0x{:04x}\nArguments are in EVM pop order.\n{:?}{}",
                row.plain_text(),
                block.state,
                instruction.pc,
                instruction.progress,
                if instruction.fault {
                    " · exceptional halt"
                } else {
                    ""
                },
            );
            rows.push(row);
        }
        let mut exit = Row::new();
        exit.token(
            format!(
                "{} μ{} · {}",
                if block.coverage == BlockCoverage::Current {
                    "exit"
                } else {
                    "entry only"
                },
                block.exit_effect,
                frames(&block.exit_frames)
            ),
            palette::MUTED,
        );
        exit.tooltip = exit.plain_text();
        rows.push(exit);
        for edge in report.edges.iter().filter(|edge| edge.from == block.state) {
            let transition = report
                .ssa
                .transitions
                .iter()
                .find(|transition| transition.edge == edge.id);
            let mut row = Row::new();
            row.target = Some(Selection {
                state: Some(edge.to),
                pc: None,
            });
            row.token(
                if let Some(transition) = transition {
                    format!(
                        "e{} → S{} · {:?} · μ{}→μ{}{}",
                        edge.id,
                        edge.to,
                        edge.kind,
                        transition.effect_input,
                        transition.effect_result,
                        transition
                            .result
                            .map_or(String::new(), |id| format!(" · %{id}"))
                    )
                } else {
                    format!(
                        "e{} → S{} · deferred {:?}",
                        edge.id,
                        edge.to,
                        report
                            .ssa
                            .deferred_edges
                            .iter()
                            .find(|deferred| deferred.edge == edge.id)
                            .map(|deferred| deferred.reason)
                    )
                },
                palette::ACCENT,
            );
            row.tooltip = if let Some(transition) = transition {
                format!(
                    "{}\nArguments: [{}]\nDestination frame stacks: {}",
                    row.plain_text(),
                    values(&transition.operands),
                    frames(&transition.stacks)
                )
            } else {
                row.plain_text()
            };
            rows.push(row);
        }
    }
    rows
}

/// The same tokens determine paint and scroll extents, so no data-bearing row
/// (including a transition or exit stack) can overrun the horizontal viewport.
#[cfg(test)]
pub(crate) fn content_columns(report: &AnalysisReport) -> usize {
    rows(report)
        .iter()
        .map(Row::columns)
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
