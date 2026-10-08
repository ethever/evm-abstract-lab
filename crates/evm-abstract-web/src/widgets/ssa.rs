//! SSA definitions, arguments, phi inputs and machine effects are painted as
//! distinct tokens. Block and instruction hit targets share native CFG IDs.

use egui::{Color32, Rect, ScrollArea, Sense, Ui, Vec2};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, InstructionProgress, SsaBlock};

use super::{ROW_HEIGHT, heading, row_background, text};
use crate::{app::Selection, palette};

pub(crate) fn ssa(
    ui: &mut Ui,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    heading(
        ui,
        "STATIC SINGLE ASSIGNMENT",
        &format!(
            "{} values · {} effects · {}",
            report.ssa.value_count,
            report.ssa.effect_count,
            if report.ssa.complete {
                "verified complete"
            } else {
                "partial coverage"
            }
        ),
    );
    let focus = *selection != *previous;
    *previous = *selection;
    ScrollArea::both()
        .id_salt("ssa_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let glyph_width = ui
                .painter()
                .layout_no_wrap("0".into(), egui::FontId::monospace(12.0), palette::TEXT)
                .size()
                .x;
            ui.set_min_width((content_columns(report) as f32 * glyph_width + 24.0).max(480.0));
            for block in &report.ssa.blocks {
                let selected = selection.state == Some(block.state);
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 42.0), Sense::click());
                ui.painter().rect_filled(
                    rect.shrink2(Vec2::new(0.0, 3.0)),
                    4.0,
                    if selected {
                        palette::SELECTED
                    } else {
                        palette::PANEL
                    },
                );
                text(
                    ui,
                    rect.left_center() + Vec2::new(10.0, 0.0),
                    format!("S{}", block.state),
                    palette::ACCENT,
                    13.0,
                );
                text(
                    ui,
                    rect.left_center() + Vec2::new(61.0, 0.0),
                    format!(
                        "{:?} · {} incoming",
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
                    11.0,
                );
                if response.clicked() {
                    *selection = Selection {
                        state: Some(block.state),
                        pc: None,
                    };
                }
                if focus && selected && selection.pc.is_none() {
                    super::focus_row(ui, response.rect);
                }
                for phi in &block.phis {
                    let rect = row(ui, ROW_HEIGHT);
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let mut cursor = rect.left_center() + Vec2::new(10.0, 0.0);
                    token(ui, &mut cursor, format!("v{}", phi.result), palette::PURPLE);
                    token(ui, &mut cursor, " = φ(", palette::MUTED);
                    for (index, input) in phi.inputs.iter().enumerate() {
                        if index > 0 {
                            token(ui, &mut cursor, ", ", palette::MUTED);
                        }
                        token(
                            ui,
                            &mut cursor,
                            format!("S{}:v{}", input.predecessor, input.value),
                            palette::BLUE,
                        );
                    }
                    token(ui, &mut cursor, ")", palette::MUTED);
                    token(
                        ui,
                        &mut cursor,
                        format!("  f{}/slot{}", phi.frame, phi.slot),
                        palette::MUTED,
                    );
                    ui.interact(
                        rect,
                        ui.id().with(("phi", block.state, phi.result)),
                        Sense::hover(),
                    )
                    .on_hover_text(format!(
                        "Frame {}, stack slot {} (bottom to top)\n{}",
                        phi.frame,
                        phi.slot,
                        phi.inputs
                            .iter()
                            .map(|input| format!(
                                "edge e{} from S{}: v{}",
                                input.edge, input.predecessor, input.value
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
                effect_entry(ui, block);
                for instruction in &block.instructions {
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
                    let selected = selection.state == Some(block.state)
                        && selection.pc == Some(instruction.pc);
                    if focus && selected {
                        super::focus_row(ui, response.rect);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    row_background(ui, rect, selected, response.hovered());
                    let mut cursor = rect.left_top() + Vec2::new(10.0, 14.0);
                    token(
                        ui,
                        &mut cursor,
                        format!("{:04x}  ", instruction.pc),
                        palette::MUTED,
                    );
                    for (index, result) in instruction.results.iter().enumerate() {
                        if index > 0 {
                            token(ui, &mut cursor, ", ", palette::MUTED);
                        }
                        token(ui, &mut cursor, format!("v{result}"), palette::PURPLE);
                    }
                    if !instruction.results.is_empty() {
                        token(ui, &mut cursor, " = ", palette::MUTED);
                    }
                    token(
                        ui,
                        &mut cursor,
                        &instruction.name,
                        if instruction.fault {
                            palette::ERROR
                        } else {
                            palette::TEXT
                        },
                    );
                    if let Some(value) = &instruction.immediate {
                        token(ui, &mut cursor, format!(" {value}"), palette::BLUE);
                    }
                    for operand in &instruction.operands {
                        token(ui, &mut cursor, format!(" v{operand}"), palette::BLUE);
                    }
                    let phase_color = if instruction.progress == InstructionProgress::Completed {
                        palette::MUTED
                    } else {
                        palette::WARNING
                    };
                    text(
                        ui,
                        rect.left_top() + Vec2::new(56.0, 34.0),
                        format!(
                            "μ{} → {} · {:?}",
                            instruction.effect_input,
                            instruction
                                .effect_result
                                .map_or_else(|| "pending".into(), |id| format!("μ{id}")),
                            instruction.progress
                        ),
                        phase_color,
                        10.0,
                    );
                    if response.clicked() {
                        *selection = Selection {
                            state: Some(block.state),
                            pc: Some(instruction.pc),
                        };
                    }
                    response.on_hover_text(format!(
                        "State S{} · pc 0x{:04x}\nArguments are in EVM pop order.\n{:?}{}",
                        block.state,
                        instruction.pc,
                        instruction.progress,
                        if instruction.fault {
                            " · exceptional halt"
                        } else {
                            ""
                        }
                    ));
                }
                let rect = row(ui, ROW_HEIGHT);
                if ui.is_rect_visible(rect) {
                    text(
                        ui,
                        rect.left_center() + Vec2::new(10.0, 0.0),
                        format!(
                            "{} μ{} · {}",
                            if block.coverage == BlockCoverage::Current {
                                "exit"
                            } else {
                                "entry only"
                            },
                            block.exit_effect,
                            block
                                .exit_frames
                                .iter()
                                .enumerate()
                                .map(|(frame, stack)| format!(
                                    "f{frame} [{}]",
                                    stack
                                        .iter()
                                        .map(|id| format!("v{id}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ))
                                .collect::<Vec<_>>()
                                .join("  ")
                        ),
                        palette::MUTED,
                        10.0,
                    );
                }
                for edge in report.edges.iter().filter(|edge| edge.from == block.state) {
                    let rect = row(ui, ROW_HEIGHT);
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let label = if let Some(transition) = report
                        .ssa
                        .transitions
                        .iter()
                        .find(|transition| transition.edge == edge.id)
                    {
                        format!(
                            "e{} → S{} · {:?} · μ{} → μ{}{}",
                            edge.id,
                            edge.to,
                            edge.kind,
                            transition.effect_input,
                            transition.effect_result,
                            transition
                                .result
                                .map_or(String::new(), |id| format!(" · v{id}"))
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
                    };
                    text(
                        ui,
                        rect.left_center() + Vec2::new(10.0, 0.0),
                        label,
                        palette::ACCENT,
                        10.0,
                    );
                    let response =
                        ui.interact(rect, ui.id().with(("transition", edge.id)), Sense::click());
                    if response.clicked() {
                        *selection = Selection {
                            state: Some(edge.to),
                            pc: None,
                        };
                    }
                    if let Some(transition) = report
                        .ssa
                        .transitions
                        .iter()
                        .find(|transition| transition.edge == edge.id)
                    {
                        response.on_hover_text(format!(
                            "Arguments: {:?}\nDestination frame stacks: {:?}",
                            transition.operands, transition.stacks
                        ));
                    }
                }
                ui.add_space(10.0);
            }
        });
}

fn row(ui: &mut Ui, height: f32) -> Rect {
    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover())
        .0
}

fn token(ui: &Ui, cursor: &mut egui::Pos2, value: impl AsRef<str>, color: Color32) {
    let bounds = text(ui, *cursor, value, color, 12.0);
    cursor.x = bounds.right();
}

fn effect_entry(ui: &mut Ui, block: &SsaBlock) {
    let rect = row(ui, ROW_HEIGHT);
    if !ui.is_rect_visible(rect) {
        return;
    }
    text(
        ui,
        rect.left_center() + Vec2::new(10.0, 0.0),
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
            }
        ),
        palette::MUTED,
        10.0,
    );
}

/// Reserve the full painted extent, including wide PUSH values and phi lists.
/// Without this, a painter can draw outside a row but ScrollArea cannot reach it.
pub(crate) fn content_columns(report: &AnalysisReport) -> usize {
    let mut columns = 70;
    for block in &report.ssa.blocks {
        for phi in &block.phis {
            let incoming: usize = phi
                .inputs
                .iter()
                .map(|input| format!("S{}:v{}, ", input.predecessor, input.value).len())
                .sum();
            columns = columns.max(
                format!("v{} = φ()  f{}/slot{}", phi.result, phi.frame, phi.slot)
                    .chars()
                    .count()
                    + incoming,
            );
        }
        for instruction in &block.instructions {
            let results: usize = instruction
                .results
                .iter()
                .map(|id| format!("v{id}, ").len())
                .sum();
            let operands: usize = instruction
                .operands
                .iter()
                .map(|id| format!(" v{id}").len())
                .sum();
            columns = columns.max(
                12 + results
                    + instruction.name.len()
                    + instruction.immediate.as_ref().map_or(0, String::len)
                    + operands,
            );
        }
        let effect_arguments: usize = block
            .effect_inputs
            .iter()
            .map(|input| {
                format!("e{}:μ{}, ", input.edge, input.effect)
                    .chars()
                    .count()
            })
            .sum();
        columns = columns.max(40 + effect_arguments + block.open_incoming.len() * 12);
        let exit_arguments: usize = block
            .exit_frames
            .iter()
            .map(|stack| {
                15 + stack
                    .iter()
                    .map(|id| format!("v{id}, ").len())
                    .sum::<usize>()
            })
            .sum();
        columns = columns.max(25 + exit_arguments);
    }
    columns
}
