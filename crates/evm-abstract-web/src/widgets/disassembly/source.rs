//! Decoded rows borrow the publication index and emit source coordinates only.
//! The shell owns selection; this cache retains only the last scroll target.

use egui::{RichText, Ui};
use evm_abstract_protocol::{AnalysisNamespace, BlockCoverage, CfgBlock, ProcessLocation};

use crate::{
    app::model::{
        ReportViewId,
        navigation::{EvidenceFocus, Focus},
        presentation::{ContextTarget, Presentation, ResolvedContext, SourceRow},
    },
    controls::{self, Density},
    palette,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourcePick {
    /// Program-local block ID, independently of its dense source-table index.
    pub(crate) block: usize,
    /// An instruction offset; block headings have no selected instruction.
    pub(crate) pc: Option<usize>,
}

impl SourcePick {
    pub(crate) fn focus(self, current: &Focus) -> Focus {
        Focus {
            evidence: EvidenceFocus::Source {
                block: Some(self.block),
                pc: self.pc,
            },
            ..current.clone()
        }
    }

    fn context_focus(self, presentation: Presentation<'_>, current: &Focus) -> Focus {
        let source = self.focus(current);
        let Some(native) = native_context(presentation, current) else {
            return source;
        };
        let state = native.state;
        if state.basic_block != Some(self.block) {
            return source;
        }
        let native = Focus {
            evidence: EvidenceFocus::Graph(crate::app::Selection {
                state: Some(state.id),
                pc: self.pc,
                value: None,
            }),
            ..current.clone()
        };
        if presentation.validate(&native).is_ok() {
            native
        } else {
            source
        }
    }
}

#[derive(Default)]
pub(crate) struct SourceCache {
    focus: Option<(ReportViewId, usize, Option<usize>, Option<usize>)>,
}

impl SourceCache {
    /// Native links require this exact state's retained instruction receipt.
    /// Other source rows remain decoded coordinates, with no chosen occurrence.
    pub(crate) fn show_context(
        &mut self,
        ui: &mut Ui,
        presentation: Presentation<'_>,
        focus: &Focus,
    ) -> Option<Focus> {
        self.show(ui, presentation, focus)
            .map(|pick| pick.context_focus(presentation, focus))
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        presentation: Presentation<'_>,
        focus: &Focus,
    ) -> Option<SourcePick> {
        // One Code pane moves between routes and responsive tiles. Its scroll
        // and popup identities must outlive those parent UI arrangements.
        ui.scope_builder(
            egui::UiBuilder::new().id(egui::Id::new("source_view")),
            |ui| self.contents(ui, presentation, focus),
        )
        .inner
    }

    fn contents(
        &mut self,
        ui: &mut Ui,
        presentation: Presentation<'_>,
        focus: &Focus,
    ) -> Option<SourcePick> {
        if let Err(error) = presentation.validate(focus) {
            crate::widgets::empty(ui, &error.to_string());
            return None;
        }
        let Some(source) = focus
            .source
            .as_ref()
            .and_then(|source| source.program())
            .and_then(|program| presentation.program(program))
        else {
            crate::widgets::empty(ui, "No captured source for this selection");
            return None;
        };
        let rows = presentation.index().source_rows(source.id)?;
        let native = native_context(presentation, focus);
        crate::widgets::heading(
            ui,
            "CODE",
            &format!(
                "{:?} · {} bytes · {} blocks",
                source.kind,
                source.bytecode.trim_start_matches("0x").len() / 2,
                source.blocks.len()
            ),
        );
        ui.horizontal_wrapped(|ui| {
            if controls::action(ui, "Copy code", Density::Compact).clicked() {
                ui.ctx().copy_text(super::copy::code(source));
            }
            if controls::action(ui, "Copy bytecode", Density::Compact).clicked() {
                ui.ctx().copy_text(source.bytecode.clone());
            }
            ui.label(
                RichText::new("Decoded source")
                    .small()
                    .color(palette::MUTED),
            );
        });
        if let Some(native) = native.filter(|native| {
            native.coverage == Some(BlockCoverage::Stale)
                || !native.at_current_revision
                || !native.ssa_complete
        }) {
            ui.horizontal_wrapped(|ui| {
                if native.coverage == Some(BlockCoverage::Stale) {
                    ui.label(
                        RichText::new("Historical Native receipt")
                            .small()
                            .color(palette::WARNING),
                    );
                }
                if !native.at_current_revision {
                    ui.label(
                        RichText::new("Earlier Native input conditions")
                            .small()
                            .color(palette::WARNING),
                    );
                }
                if !native.ssa_complete {
                    ui.label(
                        RichText::new("Partial Native SSA publication")
                            .small()
                            .color(palette::MUTED),
                    );
                }
            });
        }
        let location = focus.minimum_selection().and_then(|selection| {
            selection
                .source(presentation.report().minimum.as_deref()?)
                .filter(|(frame, _)| frame.source.matches_program(source))
                .map(|(_, location)| location)
        });
        let (block, pc) = match focus.evidence {
            EvidenceFocus::Source { block, pc } => (block, pc),
            EvidenceFocus::Graph(selection) => (
                focus.context.and_then(|context| {
                    match presentation
                        .context(ContextTarget {
                            view: focus.view,
                            context,
                        })
                        .ok()?
                    {
                        ResolvedContext::Graph { state, .. } => state.basic_block,
                        _ => None,
                    }
                }),
                selection.pc,
            ),
            EvidenceFocus::Minimum(_) => (
                match location {
                    Some(ProcessLocation::Block(index)) => {
                        source.blocks.get(index).map(|block| block.id)
                    }
                    _ => None,
                },
                None,
            ),
        };
        if let Some(location) = location {
            ui.label(
                RichText::new(format!("MAY source: {}", super::location_label(location)))
                    .small()
                    .color(palette::MUTED),
            );
        }
        let target = (focus.view, source.id, block, pc);
        let mut area = egui::ScrollArea::both()
            .id_salt(("source_rows", source.id))
            .auto_shrink([false, false]);
        if self.focus != Some(target)
            && let Some(index) =
                block.and_then(|id| source.blocks.iter().position(|block| block.id == id))
            && let Some(row) = rows.block_row(index)
        {
            let instruction = pc
                .and_then(|pc| {
                    source.blocks[index]
                        .instructions
                        .iter()
                        .position(|instruction| instruction.pc == pc)
                })
                .map_or(0, |index| index + 1);
            area = area.vertical_scroll_offset(
                (row + instruction) as f32
                    * (crate::widgets::ROW_HEIGHT + ui.spacing().item_spacing.y),
            );
        }
        self.focus = Some(target);
        let mut picked = None;
        area.show_rows(ui, crate::widgets::ROW_HEIGHT, rows.len(), |ui, range| {
            for index in range {
                let Some(row) = rows.row(source, index) else {
                    continue;
                };
                let (source_block, instruction) = match row {
                    SourceRow::Block(block) => (block, None),
                    SourceRow::Instruction { block, instruction } => (block, Some(instruction)),
                };
                let target = SourcePick {
                    block: source_block.id,
                    pc: instruction.map(|instruction| instruction.pc),
                };
                let label = instruction.map_or_else(
                    || format!("Block 0x{:04x}", source_block.start_pc),
                    super::copy::instruction,
                );
                // Fixed-height, unwrapped rows retain full PUSH32 text and both
                // scroll axes, while only the visible range is formatted.
                let selected =
                    block == Some(target.block) && (target.pc.is_none() || target.pc == pc);
                let response = ui.add(
                    egui::Button::selectable(
                        selected,
                        RichText::new(&label)
                            .monospace()
                            .color(if instruction.is_none() {
                                palette::TEXT
                            } else {
                                palette::MUTED
                            }),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                if response.clicked() {
                    picked = Some(target);
                }
                response.context_menu(|ui| {
                    if ui.button("Copy row").clicked() {
                        ui.ctx().copy_text(label.clone());
                        ui.close();
                    }
                    if ui.button("Copy source / PC reference").clicked() {
                        ui.ctx().copy_text(format!(
                            "{}\nBlock: {}\nPublication: {:?}\nContext: {:?}",
                            super::copy::source_reference(source, focus.view, target.pc, None),
                            target.block,
                            presentation.identity(),
                            focus.context
                        ));
                        ui.close();
                    }
                });
                if let Some(instruction) = instruction {
                    let executed = native.is_some_and(|native| {
                        native.coverage == Some(BlockCoverage::Current)
                            && native.at_current_revision
                            && native.state.basic_block == Some(source_block.id)
                            && native.state.executed_pcs.contains(&instruction.pc)
                    });
                    response.on_hover_text(super::tooltip::instruction(instruction, executed));
                }
            }
        });
        picked
    }
}

#[derive(Clone, Copy)]
struct NativeContext<'a> {
    state: &'a CfgBlock,
    coverage: Option<BlockCoverage>,
    at_current_revision: bool,
    ssa_complete: bool,
}

fn native_context<'a>(presentation: Presentation<'a>, focus: &Focus) -> Option<NativeContext<'a>> {
    if focus.view.namespace() != AnalysisNamespace::Native {
        return None;
    }
    match presentation
        .context(ContextTarget {
            view: focus.view,
            context: focus.context?,
        })
        .ok()?
    {
        ResolvedContext::Graph {
            graph,
            state,
            coverage,
            at_current_revision,
            ..
        } if state.process.is_none() => Some(NativeContext {
            state,
            coverage,
            at_current_revision,
            ssa_complete: graph.ssa.complete,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
