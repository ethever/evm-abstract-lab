//! SSA uses compact virtual table rows. Tokens retain semantic coloring while
//! native state/PC targets keep table selection linked to the graph and source.
use crate::report::ReportView;

use evm_abstract_notation::Symbol;
mod cache;

pub(crate) use cache::SsaCache;

use std::collections::{BTreeMap, BTreeSet};

use egui::{Color32, FontId, ScrollArea, Sense, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use evm_abstract_protocol::{BlockCoverage, InstructionProgress};

use super::{ROW_HEIGHT, heading, row_background, text};
use crate::{
    app::Selection,
    palette,
    ssa_display::{DisplayOperand, SsaDisplay, operation_parts, source_text},
};

pub(crate) fn ssa_cached(
    ui: &mut Ui,
    report: &ReportView<'_>,
    program: Option<usize>,
    display: SsaDisplay,
    selection: &mut Selection,
    previous: &mut Selection,
    cache: &mut SsaCache,
) {
    // Exactly one instance is visible at a time, whether standalone or inside
    // responsive tiles. Its absolute UI identity preserves both scroll axes
    // when the parent tree changes, without overriding manual navigation.
    ui.scope_builder(egui::UiBuilder::new().id(egui::Id::new("ssa_view")), |ui| {
        contents(ui, report, program, display, selection, previous, cache)
    });
}

fn contents(
    ui: &mut Ui,
    report: &ReportView<'_>,
    program: Option<usize>,
    display: SsaDisplay,
    selection: &mut Selection,
    previous: &mut Selection,
    cache: &mut SsaCache,
) {
    if report.identity().namespace() != evm_abstract_protocol::AnalysisNamespace::Native {
        heading(ui, "SSA", "Conservative control");
        super::empty(ui, "MAY control has no Native instruction or SSA receipt");
        return;
    }
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
    ui.horizontal_wrapped(|ui| {
        crate::controls::tab_value(
            ui,
            &mut cache.mode,
            cache::ScopeMode::State,
            "Selected state",
            crate::controls::Density::Compact,
        );
        crate::controls::tab_value(
            ui,
            &mut cache.mode,
            cache::ScopeMode::Source,
            "All source states",
            crate::controls::Density::Compact,
        );
    });
    cache.prepare(report, program, display, *selection);
    ui.horizontal(|ui| {
        let response = ui
            .add_enabled(!cache.rows().is_empty(), egui::Button::new("Copy SSA"))
            .on_hover_text(
                "Copy complete SSA for this scope, including values, effects and transitions",
            );
        crate::controls::observe(ui, &response, "Copy SSA", "action", None);
        if response.clicked() {
            ui.ctx().copy_text(cache.full_text(report, *selection));
        }
    });
    if cache.rows().is_empty() {
        super::empty(
            ui,
            if selection.state.is_none() && cache.mode == cache::ScopeMode::State {
                "Select a detailed state to inspect its SSA, or choose all source states."
            } else {
                "SSA unavailable for this source and context. No instruction receipt is recorded here."
            },
        );
        return;
    }
    let rows = cache.rows();
    let focus = (!selection.same_native(*previous))
        .then(|| {
            rows.iter().position(|row| {
                row.target
                    .is_some_and(|target| target.same_native(*selection))
                    && (selection.pc.is_some() || row.header)
            })
        })
        .flatten();
    *previous = *selection;
    // Width comes from the actual tokens, including effects, exit stacks and
    // transitions. A short report therefore fits a narrow pane without a
    // permanent blank gutter; long operands remain horizontally reachable.
    let width = row_width(ui, cache.columns());
    ScrollArea::horizontal()
        .id_salt("ssa_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_min_width(width);
            let height = ui.available_height();
            let observation_clip = ui.clip_rect();
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
                    let mut value_click = None;
                    table_row.col(|ui| {
                        let rect = ui.max_rect();
                        if row.header {
                            ui.painter().rect_filled(rect, 0.0, palette::PANEL);
                        }
                        row_background(ui, rect, selected, ui.rect_contains_pointer(rect));
                        let mut cursor = rect.left_center() + Vec2::new(4.0, 0.0);
                        for (index, token) in row.tokens.iter().enumerate() {
                            let rect = text(ui, cursor, &token.text, token.color, 12.0);
                            cursor.x = rect.right();
                            if let Some(value) = token.value {
                                if selection.value == Some(value) {
                                    ui.painter().line_segment(
                                        [rect.left_bottom(), rect.right_bottom()],
                                        egui::Stroke::new(1.0, palette::ACCENT),
                                    );
                                }
                                let response = ui.interact(
                                    rect,
                                    ui.id().with(("ssa_value", index)),
                                    Sense::click(),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Link,
                                        true,
                                        format!("Inspect SSA value %{value}"),
                                    )
                                });
                                if response.clicked() {
                                    value_click = Some(value);
                                }
                                response
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text(format!("Inspect SSA value %{value}"));
                            }
                        }
                    });
                    let response = table_row.response();
                    crate::controls::observe_response(
                        &response,
                        observation_clip,
                        &row.plain_text(),
                        "ssa-row",
                        Some(selected),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            selected,
                            row.plain_text(),
                        )
                    });
                    if let Some(value) = value_click {
                        *selection = Selection {
                            value: Some(value),
                            ..row.target.unwrap_or(*selection)
                        };
                    } else if response.clicked()
                        && let Some(target) = row.target
                    {
                        *selection = target;
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy row").clicked() {
                            ui.ctx().copy_text(row.plain_text());
                            ui.close();
                        }
                        if ui.button("Copy full details").clicked() {
                            ui.ctx().copy_text(row.tooltip.clone());
                            ui.close();
                        }
                        if let Some(target) = row.target
                            && let Some(state) = report
                                .cfg
                                .iter()
                                .find(|state| Some(state.id) == target.state)
                            && let Some(source) = report
                                .programs
                                .iter()
                                .find(|source| Some(source.id) == state.program)
                            && ui.button("Copy source / PC reference").clicked()
                        {
                            ui.ctx().copy_text(super::disassembly::source_reference(
                                report, source, target,
                            ));
                            ui.close();
                        }
                    });
                    response.on_hover_ui(|ui| {
                        ui.label(crate::notation::widget(
                            ui,
                            egui::RichText::new(&row.tooltip).monospace().size(12.0),
                        ));
                    });
                });
            });
        });
}

/// Measure every retained native block in the active scope, including phi
/// inputs, effects and transitions. Graph folding never folds these SSA rows.
pub(crate) fn natural_width_cached(
    ui: &Ui,
    report: &ReportView<'_>,
    program: Option<usize>,
    display: SsaDisplay,
    selection: Selection,
    cache: &mut SsaCache,
) -> f32 {
    cache.prepare(report, program, display, selection);
    row_width(ui, cache.columns()) + ui.spacing().scroll.allocated_width()
}

fn row_width(ui: &Ui, columns: usize) -> f32 {
    let glyph_width = ui
        .painter()
        .layout_no_wrap("0".into(), FontId::monospace(12.0), palette::TEXT)
        .size()
        .x;
    columns as f32 * glyph_width + 12.0
}

#[cfg(test)]
pub(crate) fn ssa(
    ui: &mut Ui,
    report: &ReportView<'_>,
    program: Option<usize>,
    display: SsaDisplay,
    selection: &mut Selection,
    previous: &mut Selection,
) {
    ssa_cached(
        ui,
        report,
        program,
        display,
        selection,
        previous,
        &mut SsaCache::default(),
    );
}

struct Token {
    text: String,
    color: Color32,
    value: Option<usize>,
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
            value: None,
        });
    }

    fn value(&mut self, value: usize, color: Color32) {
        self.value_label(format!("%{value}"), value, color);
    }

    fn value_label(&mut self, text: String, value: usize, color: Color32) {
        self.tokens.push(Token {
            text,
            color,
            value: Some(value),
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
        .map(|(frame, stack)| {
            format!(
                "{notation_0} [{}]",
                values(stack),
                notation_0 = Symbol::Frame(frame)
            )
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn scoped_states(
    report: &ReportView<'_>,
    program: Option<usize>,
    selection: Selection,
) -> BTreeSet<usize> {
    // Program membership is independent of SSA receipt count. Compute it once
    // so many context instances do not each scan the entire native CFG.
    report
        .cfg
        .iter()
        .filter(|block| {
            block.program == program && (program.is_some() || selection.state == Some(block.id))
        })
        .map(|block| block.id)
        .collect()
}

#[cfg(test)]
fn scoped_rows(
    report: &ReportView<'_>,
    program: Option<usize>,
    display: SsaDisplay,
    selection: Selection,
) -> Vec<Row> {
    let included = scoped_states(report, program, selection);
    build_rows(report, display, selection, |state| {
        included.contains(&state)
    })
}

#[cfg(test)]
fn rows(report: &ReportView<'_>) -> Vec<Row> {
    build_rows(report, SsaDisplay::Full, Selection::default(), |_| true)
}

fn build_rows(
    report: &ReportView<'_>,
    display: SsaDisplay,
    selection: Selection,
    include: impl Fn(usize) -> bool,
) -> Vec<Row> {
    let mut outgoing = BTreeMap::<_, Vec<_>>::new();
    for edge in &report.edges {
        outgoing.entry(edge.from).or_default().push(edge);
    }
    let transitions: BTreeMap<_, _> = report
        .ssa
        .transitions
        .iter()
        .map(|transition| (transition.edge, transition))
        .collect();
    let deferred: BTreeMap<_, _> = report
        .ssa
        .deferred_edges
        .iter()
        .map(|edge| (edge.edge, edge.reason))
        .collect();
    let mut rows = Vec::new();
    for block in report
        .ssa
        .blocks
        .iter()
        .filter(|block| include(block.state))
    {
        let mut header = Row::new();
        header.header = true;
        header.target = Some(Selection {
            value: None,
            state: Some(block.state),
            pc: None,
        });
        header.token(Symbol::State(block.state).to_string(), palette::ACCENT);
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
            row.target = Some(Selection {
                state: Some(block.state),
                ..Selection::default()
            });
            row.value(phi.result, palette::PURPLE);
            row.token(" = φ(", palette::MUTED);
            for (index, input) in phi.inputs.iter().enumerate() {
                if index > 0 {
                    row.token(", ", palette::MUTED);
                }
                row.value_label(
                    format!("{}:%{}", Symbol::State(input.predecessor), input.value),
                    input.value,
                    palette::BLUE,
                );
            }
            row.token(")", palette::MUTED);
            row.token(
                format!("  {}/slot{}", Symbol::Frame(phi.frame), phi.slot),
                palette::MUTED,
            );
            row.tooltip = format!(
                "{}\nFrame {}, stack slot {} (bottom to top)\n{}",
                row.plain_text(),
                Symbol::Frame(phi.frame),
                phi.slot,
                phi.inputs
                    .iter()
                    .map(|input| format!(
                        "edge {} from {}: %{}",
                        Symbol::Edge(input.edge),
                        Symbol::State(input.predecessor),
                        input.value
                    ))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            rows.push(row);
        }
        let mut effect = Row::new();
        effect.token(
            format!(
                "{} = effect φ({}){}",
                Symbol::Effect(block.effect),
                block
                    .effect_inputs
                    .iter()
                    .map(|input| format!(
                        "{}:{}",
                        Symbol::Edge(input.edge),
                        Symbol::Effect(input.effect)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                if block.open_incoming.is_empty() {
                    String::new()
                } else {
                    format!(
                        " · open [{}]",
                        block
                            .open_incoming
                            .iter()
                            .map(|edge| Symbol::Edge(*edge).to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            ),
            palette::MUTED,
        );
        effect.tooltip = effect.plain_text();
        rows.push(effect);
        for instruction in &block.instructions {
            let target = Selection {
                value: None,
                state: Some(block.state),
                pc: Some(instruction.pc),
            };
            if !display.instruction_visible(
                block.coverage,
                instruction,
                target.same_native(selection),
            ) {
                continue;
            }
            let mut row = Row::new();
            row.target = Some(target);
            row.token(format!("{:04x}  ", instruction.pc), palette::MUTED);
            for (index, result) in instruction.results.iter().enumerate() {
                if index > 0 {
                    row.token(", ", palette::MUTED);
                }
                row.value(*result, palette::PURPLE);
            }
            if !instruction.results.is_empty() {
                row.token(" = ", palette::MUTED);
            }
            let (name, operands) = operation_parts(&instruction.operation);
            row.token(
                name,
                if instruction.fault {
                    palette::ERROR
                } else {
                    palette::TEXT
                },
            );
            for operand in operands {
                row.token(" ", palette::BLUE);
                match operand {
                    DisplayOperand::Value(value) => row.value(value, palette::BLUE),
                    DisplayOperand::Constant(word) => row.token(word, palette::BLUE),
                }
            }
            let effect = format!(
                " · {}→{}",
                Symbol::Effect(instruction.effect_input),
                instruction
                    .effect_result
                    .map_or_else(|| "pending".into(), |id| Symbol::Effect(id).to_string())
            );
            // Compact rows retain all instruction detail in the tooltip, even
            // when their ordinary machine-effect chain is visually omitted.
            let detail = format!("{}{effect}", row.plain_text());
            if display.show_effect(instruction) {
                row.token(
                    effect,
                    if instruction.progress == InstructionProgress::Completed {
                        palette::MUTED
                    } else {
                        palette::WARNING
                    },
                );
            }
            if display.show_completed()
                || instruction.progress != InstructionProgress::Completed
                || instruction.fault
            {
                row.token(
                    format!(" · {:?}", instruction.progress),
                    if instruction.fault {
                        palette::ERROR
                    } else if instruction.progress == InstructionProgress::Completed {
                        palette::MUTED
                    } else {
                        palette::WARNING
                    },
                );
            }
            row.tooltip = format!(
                "{}\nState {} · pc 0x{:04x}\n{}\nArguments are in EVM pop order.\n{:?}{}",
                detail,
                Symbol::State(block.state),
                instruction.pc,
                source_text(instruction),
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
                "{} {} · {}",
                if block.coverage == BlockCoverage::Current {
                    "exit"
                } else {
                    "entry only"
                },
                Symbol::Effect(block.exit_effect),
                frames(&block.exit_frames)
            ),
            palette::MUTED,
        );
        exit.tooltip = exit.plain_text();
        rows.push(exit);
        for edge in outgoing.get(&block.state).into_iter().flatten() {
            let transition = transitions.get(&edge.id).copied();
            let mut row = Row::new();
            row.target = Some(Selection {
                value: None,
                state: Some(edge.to),
                pc: None,
            });
            row.token(
                if let Some(transition) = transition {
                    format!(
                        "{} → {} · {:?} · {}→{}{}",
                        Symbol::Edge(edge.id),
                        Symbol::State(edge.to),
                        edge.kind,
                        Symbol::Effect(transition.effect_input),
                        Symbol::Effect(transition.effect_result),
                        transition
                            .result
                            .map_or(String::new(), |id| format!(" · %{id}"))
                    )
                } else {
                    format!(
                        "{} → {} · deferred {:?}",
                        Symbol::Edge(edge.id),
                        Symbol::State(edge.to),
                        deferred.get(&edge.id)
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
pub(crate) fn content_columns(report: &ReportView<'_>) -> usize {
    rows(report)
        .iter()
        .map(Row::columns)
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
