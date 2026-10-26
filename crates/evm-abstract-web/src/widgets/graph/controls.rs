//! View controls and native-instance selection. The graph owns display choices;
//! shared selection always remains an original analysis state ID.

use super::{Graph, NativeIntent, NodeView};
use crate::controls::{self, Density};
use crate::report::ReportView;
use crate::{app::Selection, palette, widgets::GraphMode};
use egui::{Context, RichText, Ui};
use evm_abstract_notation::Symbol;

const TERMINATION_LEGEND: &str = "CFG termination colors\nDeep green: normal end / return\nDeep purple: REVERT\nDeep red: exception\nNeutral: mixed, continuing, or no complete termination evidence\nRoot end and Frame return labels show native state counts.\nDashed amber outline: unresolved frontier / non-current coverage\nTurquoise outer outline: selected state\nLow zoom shows geometry; zoom in for source labels and previews.";

impl Graph {
    pub(super) fn controls(
        &mut self,
        ui: &mut Ui,
        report: &ReportView<'_>,
        selection: &mut Selection,
    ) {
        let mut content = self.content;
        let selected_visible = selection
            .state
            .is_some_and(|state| self.scene.display.state_to_node.contains_key(&state));
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                controls::tab_value(ui, &mut content, NodeView::Disassembly, "Disasm", Density::Compact)
                    .on_hover_text(crate::notation::widget(ui, "Decoded source instructions"));
                controls::tab_value(ui, &mut content, NodeView::Ssa, "SSA", Density::Compact)
                    .on_hover_text(crate::notation::widget(ui, "Original SSA for individual states; aggregate cards summarize their instances"));
                if controls::action(ui, "Fit graph", Density::Compact)
                    .on_hover_text(crate::notation::widget(ui, "F / Shift+F: center the existing scene"))
                    .clicked()
                {
                    self.request_fit(ui, true);
                }
                if controls::action_enabled(ui, selected_visible, "Focus selected", Density::Compact)
                    .clicked()
                {
                    self.focus_selection();
                }
                if controls::action(ui, "Entry", Density::Compact).clicked() {
                    self.entry_pending = true;
                    self.fit_pending = false;
                }
                ui.label(
                    crate::notation::widget(ui, RichText::new(format!(
                        "{:.0}% · {}",
                        self.camera().0 * 100.0,
                        if self.fitted { "Fit" } else { "Manual" }
                    ))
                    .small()
                    .color(palette::MUTED)),
                )
                    .on_hover_text(crate::notation::widget(ui, format!("Drag or scroll to pan; pinch or Ctrl/Cmd+scroll to zoom. Resizing preserves the scene and camera.\n\n{TERMINATION_LEGEND}")));
                // Do not create another wrapped toolbar row on a narrow canvas.
                // The existing zoom label also exposes the full legend on hover.
                if ui.available_width() >= 70.0 {
                    ui.label(crate::notation::widget(ui, RichText::new("End colors").small().color(palette::MUTED)))
                        .on_hover_text(crate::notation::widget(ui, TERMINATION_LEGEND));
                }
            });
        });
        self.set_content(content, selection.state);
        let mut mode = self.mode;
        let mut scope = self.scene.program_scope;
        let mut hops = self.neighborhood_hops;
        let mut whole_scope = self.whole_scope;
        Density::Compact.scope(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                controls::tab_value(ui, &mut mode, GraphMode::Blocks, "Blocks", Density::Compact)
                    .on_hover_text(crate::notation::widget(ui, "Each program basic block appears once. Summary paths can combine different instances; choose a state for its actual SSA."));
                controls::tab_value(ui, &mut mode, GraphMode::States, "States", Density::Compact)
                    .on_hover_text(crate::notation::widget(ui, "Original Native states in this scoped window. Overview expands the selected source across invocation contexts."));
                ui.checkbox(&mut whole_scope, "Overview")
                    .on_hover_text("Expand the selected source across invocation contexts. Cards retain independent Native instances.");
                ui.add_enabled_ui(selection.state.is_some(), |ui| {
                    controls::tab_value(ui, &mut mode, GraphMode::Local, "Local", Density::Compact)
                        .on_hover_text(crate::notation::widget(ui, "Recorded adjacent states. Separate invocation contexts and out-of-window targets remain navigable boundaries; Overview can show up to 200 adjacent states."));
                });
                if mode == GraphMode::Local {
                    ui.label(
                        crate::notation::widget(ui, RichText::new(if self.whole_scope { "Cross-program neighborhood" } else { "Adjacent control" })
                            .small()
                            .color(palette::MUTED)),
                    );
                    ui.label(crate::notation::widget(ui, "Hops"));
                    ui.add(egui::DragValue::new(&mut hops).range(1..=4));
                } else {
                    egui::ComboBox::from_id_salt("cfg_program_scope")
                        .selected_text(crate::notation::widget(ui, scope.and_then(|id| report.programs.iter().find(|program| program.id == id)).map_or("All sources".into(), |program| program_caption(program))))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            controls::tab_value(ui, &mut scope, None, "All programs", Density::Compact);
                            for program in report.programs {
                                controls::tab_value(ui,
                                    &mut scope,
                                    Some(program.id),
                                    program_caption(program), Density::Compact)
                                .on_hover_text(crate::notation::widget(ui, format!(
                                    "{}\n{}", program.code_address.as_deref().unwrap_or("Constructor source"), program.code_hash
                                )));
                            }
                        });
                }
                let selected_group = selection
                    .state
                    .and_then(|state| self.scene.display.state_to_node.get(&state).copied());
                let multiple = selected_group
                    .and_then(|id| self.scene.node_positions.get(&id))
                    .and_then(|position| self.scene.display.nodes.get(*position))
                    .is_some_and(|node| node.members.len() > 1);
                if controls::action_enabled(ui, multiple, "Instances", Density::Compact)
                    .clicked()
                {
                    self.scene.instance_group = selected_group;
                    self.scene.instance_filter.clear();
                }
                if let Some(state) = selection.state {
                    ui.label(
                        crate::notation::widget(ui, RichText::new(format!(
                            "{}{}",
                            self.scene.index.as_ref().and_then(|index| index.cfg(report, state)).and_then(|state| state.start_pc).map_or("Selected control".into(), |pc| format!("Selected source 0x{pc:04x}")),
                            if selected_visible { "" } else { " · outside view" }
                        ))
                        .small()
                        .color(palette::MUTED)),
                    );
                }
            });
        });
        self.set_mode(mode);
        if self.whole_scope != whole_scope {
            self.whole_scope = whole_scope;
            self.invalidate_scene();
        }
        if scope != self.scene.program_scope {
            self.set_program_scope(scope);
        }
        if hops != self.neighborhood_hops {
            self.neighborhood_hops = hops;
            self.invalidate_scene();
        }
    }

    /// The new projection must be ready before reserving this row, so its
    /// first fit uses the same canvas bounds as subsequent frames.
    pub(super) fn view_legend(&self, ui: &mut Ui) {
        if self.scene.display.hidden_states > 0 || self.scene.display.boundary_edges > 0 {
            ui.label(
                crate::notation::widget(ui, RichText::new(format!(
                    "View hides {} states · {} boundary edges",
                    self.scene.display.hidden_states, self.scene.display.boundary_edges
                ))
                .small()
                .color(palette::MUTED)),
            )
            .on_hover_text(crate::notation::widget(ui, "This is a display subset. The analysis result and its completion status are unchanged."));
        }
    }

    pub(super) fn instances(
        &mut self,
        ctx: &Context,
        report: &ReportView<'_>,
        selection: &mut Selection,
    ) -> Option<NativeIntent> {
        let group_id = self.scene.instance_group?;
        let Some(node) = self
            .scene
            .node_positions
            .get(&group_id)
            .and_then(|position| self.scene.display.nodes.get(*position))
        else {
            self.scene.instance_group = None;
            return None;
        };
        let Some(index) = &self.scene.index else {
            return None;
        };
        let portal = self.scene.display.portals.get(&group_id);
        let members = portal.map_or(node.members.as_slice(), |portal| portal.targets.as_slice());
        let mut open = true;
        let mut picked = None;
        egui::Window::new("State instances")
            .id(egui::Id::new("cfg_instances"))
            .open(&mut open)
            .default_width(460.0)
            .max_width((ctx.content_rect().width() - 24.0).max(180.0))
            .show(ctx, |ui| {
                if portal.is_some() { ui.label("Recorded destinations · choose a separate invocation"); }
                else { ui.label(crate::notation::widget(ui, format!(
                    "{}:{} · {} state instances",
                    node.program.map_or("Native".into(), |id| Symbol::Program(id).to_string()),
                    node.basic_block.map_or_else(|| "Process control".into(), |id| Symbol::Block(id).to_string()),
                    node.members.len()
                ))); }
                ui.label(
                    crate::notation::widget(ui, RichText::new("Choose an original state to view its SSA, stack and effects. This overview does not merge their execution contexts.")
                        .small()
                        .color(palette::MUTED)),
                );
                ui.horizontal(|ui| {
                    ui.label(crate::notation::widget(ui, "Find state"));
                    crate::framework::line_editor(
                        ui,
                        "cfg_instance_search",
                        &mut self.scene.instance_filter,
                        "σ₁₂₈ or numeric state ID",
                    );
                });
                let query = self.scene.instance_filter.trim();
                let selected_id = evm_abstract_notation::parse_state(query);
                let matching = selected_id.filter(|state| members.contains(state));
                let rows = if query.is_empty() {
                    members.len()
                } else {
                    usize::from(matching.is_some())
                };
                if rows == 0 {
                    ui.label(crate::notation::widget(ui, "No matching state in this block"));
                }
                egui::ScrollArea::vertical()
                    .id_salt("cfg_instance_rows")
                    .max_height((ctx.content_rect().height() * 0.45).min(360.0))
                    .show_rows(ui, 22.0, rows, |ui, range| {
                        for row in range {
                            let state = matching.unwrap_or_else(|| members[row]);
                            let Some(block) = index.cfg(report, state) else {
                                continue;
                            };
                            let coverage = index.ssa(report, state).map_or(
                                evm_abstract_protocol::BlockCoverage::Unexecuted,
                                |block| block.coverage,
                            );
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            let label = format!(
                                "{notation_0} · {coverage:?} · depth {} · stack {} · {} jumps",
                                block.frame_depth, block.entry_stack.len(), block.context.len()
                            , notation_0 = Symbol::State(state));
                            let tooltip = format!(
                                "Code: {}\nStorage: {}\nContext: {:?}",
                                report.code_label(block), report.storage_label(block), block.context
                            );
                            if ui
                                .selectable_label(selection.state == Some(state), crate::notation::widget(ui, label))
                                .on_hover_text(crate::notation::widget(ui, tooltip))
                                .clicked()
                            {
                                picked = Some(state);
                            }
                        }
                    });
            });
        let mut intent = None;
        if let Some(state) = picked {
            if portal.is_some() {
                self.open_native_state(report, selection, state);
                intent = Some(NativeIntent::Open(*selection));
            } else {
                *selection = Selection {
                    value: None,
                    state: Some(state),
                    pc: None,
                };
            }
            if intent.is_none() {
                intent = Some(NativeIntent::Select(*selection));
            }
            open = false;
            ctx.request_repaint();
        }
        if !open {
            self.scene.instance_group = None;
        }
        intent
    }
}

fn program_caption(program: &evm_abstract_protocol::ProgramInfo) -> String {
    program.code_address.as_deref().map_or_else(
        || format!("{:?} code", program.kind),
        |address| {
            if address.len() > 18 && address.is_ascii() {
                format!("{}…{}", &address[..8], &address[address.len() - 6..])
            } else {
                address.into()
            }
        },
    )
}
