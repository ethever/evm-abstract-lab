//! Opcode rows paint separate address, mnemonic, immediate and stack columns.

use egui::{Rect, ScrollArea, Sense, Ui, Vec2};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, DisasmInstruction};

use super::{ROW_HEIGHT, badge, heading, row_background, text};
use crate::{app::Selection, palette};

pub(crate) fn disassembly(
    ui: &mut Ui,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    let active = report
        .cfg
        .iter()
        .find(|block| Some(block.id) == selection.state);
    let child = active.filter(|block| block.frame_depth > 1);
    heading(
        ui,
        "DISASSEMBLY",
        &child.map_or_else(
            || {
                format!(
                    "Root program · {} bytes · {:?}",
                    report.byte_len, report.fork
                )
            },
            |block| {
                format!(
                    "Selected frame · depth {} · S{}",
                    block.frame_depth, block.id
                )
            },
        ),
    );
    let focus = *selection != *previous;
    *previous = *selection;
    ScrollArea::both()
        .id_salt("disasm_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(340.0);
            let (header, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::hover());
            text(
                ui,
                header.left_center() + Vec2::new(9.0, 0.0),
                "PC",
                palette::MUTED,
                10.0,
            );
            text(
                ui,
                header.left_center() + Vec2::new(76.0, 0.0),
                "OPCODE / OPERAND",
                palette::MUTED,
                10.0,
            );
            text(
                ui,
                header.left_center() + Vec2::new(272.0, 0.0),
                "IN → OUT",
                palette::MUTED,
                10.0,
            );
            if let Some(block) = child {
                let Some(start_pc) = block.start_pc else {
                    super::empty(ui, "No bytecode in selected frame");
                    return;
                };
                block_header(
                    ui,
                    block.basic_block,
                    start_pc,
                    Some(block.id) == selection.state,
                );
                for instruction in &block.instructions {
                    instruction_row(
                        ui,
                        instruction,
                        Some(block.id),
                        Some(block.id) == selection.state,
                        super::coverage(report, block.id) == BlockCoverage::Current
                            && block.executed_pcs.contains(&instruction.pc),
                        selection,
                        focus,
                    );
                }
            } else {
                for block in &report.disassembly {
                    let selected_state = report.cfg.iter().find(|state| {
                        state.frame_depth == 1
                            && state.basic_block == block.id
                            && Some(state.id) == selection.state
                    });
                    let state = selected_state.or_else(|| {
                        report
                            .cfg
                            .iter()
                            .find(|state| state.frame_depth == 1 && state.basic_block == block.id)
                    });
                    let selected = selected_state.is_some();
                    let response = block_header(ui, block.id, block.start_pc, selected);
                    if response.clicked() {
                        *selection = Selection {
                            state: state.map(|state| state.id),
                            pc: Some(block.start_pc),
                        };
                    }
                    if focus && selected && selection.pc.is_none() {
                        super::focus_row(ui, response.rect);
                    }
                    for instruction in &block.instructions {
                        instruction_row(
                            ui,
                            instruction,
                            state.map(|state| state.id),
                            selected,
                            state.is_some_and(|state| {
                                super::coverage(report, state.id) == BlockCoverage::Current
                                    && state.executed_pcs.contains(&instruction.pc)
                            }),
                            selection,
                            focus,
                        );
                    }
                }
            }
        });
}

fn block_header(ui: &mut Ui, id: usize, pc: usize, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    ui.painter()
        .rect_filled(rect.shrink2(Vec2::new(0.0, 3.0)), 3.0, palette::PANEL);
    text(
        ui,
        rect.left_center() + Vec2::new(10.0, 0.0),
        format!("B{id}  ·  0x{pc:04x}"),
        if selected {
            palette::ACCENT
        } else {
            palette::MUTED
        },
        11.0,
    );
    response
}

fn instruction_row(
    ui: &mut Ui,
    instruction: &DisasmInstruction,
    state: Option<usize>,
    selected_block: bool,
    executed: bool,
    selection: &mut Selection,
    focus: bool,
) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let selected = selection.pc == Some(instruction.pc)
        && (selected_block || (selection.state.is_none() && state.is_none()));
    if focus && selected {
        super::focus_row(ui, response.rect);
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    row_background(ui, rect, selected, response.hovered());
    let origin = rect.left_center();
    text(
        ui,
        origin + Vec2::new(9.0, 0.0),
        format!("{:04x}", instruction.pc),
        palette::MUTED,
        12.0,
    );
    let color = if !instruction.valid {
        palette::ERROR
    } else if executed {
        palette::TEXT
    } else {
        palette::MUTED
    };
    text(
        ui,
        origin + Vec2::new(76.0, 0.0),
        &instruction.name,
        color,
        12.0,
    );
    if let Some(immediate) = &instruction.immediate {
        let preview = if immediate.len() > 13 {
            format!("{}…", &immediate[..12])
        } else {
            immediate.clone()
        };
        text(
            ui,
            origin + Vec2::new(151.0, 0.0),
            preview,
            palette::BLUE,
            11.0,
        );
    }
    badge(
        ui,
        Rect::from_center_size(origin + Vec2::new(299.0, 0.0), Vec2::new(54.0, 19.0)),
        &format!(
            "{} → {}",
            instruction.stack_inputs, instruction.stack_outputs
        ),
        palette::MUTED,
    );
    if response.clicked() {
        *selection = Selection {
            state,
            pc: Some(instruction.pc),
        };
    }
    response.on_hover_text(format!(
        "0x{:04x} · opcode 0x{:02x} · {} encoded bytes\n{}{}\n{}",
        instruction.pc,
        instruction.opcode,
        instruction.size,
        instruction.name,
        instruction
            .immediate
            .as_ref()
            .map_or(String::new(), |value| format!(" {value}")),
        if executed {
            "Observed in the selected state"
        } else {
            "Decoded source; no current execution receipt for this instruction"
        }
    ));
}
