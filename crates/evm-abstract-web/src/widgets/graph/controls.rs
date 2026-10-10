//! View controls and native-instance selection. The graph owns display choices;
//! shared selection always remains an original analysis state ID.

use super::{Graph, NodeView};
use crate::{app::Selection, palette, widgets::GraphMode};
use egui::{Context, RichText, Ui};
use evm_abstract_protocol::AnalysisReport;

impl Graph {
    pub(super) fn controls(
        &mut self,
        ui: &mut Ui,
        report: &AnalysisReport,
        selection: &mut Selection,
    ) {
        let mut content = self.content;
        let selected_visible = selection
            .state
            .is_some_and(|state| self.display.state_to_node.contains_key(&state));
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut content, NodeView::Disassembly, "Disasm")
                .on_hover_text("Decoded source instructions");
            ui.selectable_value(&mut content, NodeView::Ssa, "SSA")
                .on_hover_text("Original SSA for individual states; aggregate cards summarize their instances");
            if ui
                .small_button("Fit graph")
                .on_hover_text("F / Shift+F: center the existing scene")
                .clicked()
            {
                self.request_fit(ui, true);
            }
            if ui
                .add_enabled(selected_visible, egui::Button::new("Focus selected").small())
                .clicked()
            {
                self.focus_pending = true;
            }
            ui.label(
                RichText::new(format!(
                    "{:.0}% · {}",
                    self.zoom * 100.0,
                    if self.fitted { "Fit" } else { "Manual" }
                ))
                .small()
                .color(palette::MUTED),
            )
                .on_hover_text("Drag or scroll to pan; pinch or Ctrl/Cmd+scroll to zoom. Resizing preserves the scene and camera.");
        });
        self.set_content(content, selection.state);
        let mut mode = self.mode;
        let mut scope = self.program_scope;
        let mut hops = self.neighborhood_hops;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut mode, GraphMode::Blocks, "Blocks")
                .on_hover_text("Each program basic block appears once. Summary paths can combine different instances; choose a state for its actual SSA.");
            ui.selectable_value(&mut mode, GraphMode::States, "States")
                .on_hover_text("Every original native analysis state and edge");
            ui.add_enabled_ui(selection.state.is_some(), |ui| {
                ui.selectable_value(&mut mode, GraphMode::Local, "Local")
                    .on_hover_text("Recorded predecessors and successors of the selected state, including calls across programs; at most 200 states");
            });
            if mode == GraphMode::Local {
                ui.label(
                    RichText::new("Cross-program neighborhood")
                        .small()
                        .color(palette::MUTED),
                );
                ui.label("Hops");
                ui.add(egui::DragValue::new(&mut hops).range(1..=4));
            } else {
                egui::ComboBox::from_id_salt("cfg_program_scope")
                    .selected_text(scope.map_or("All programs".into(), |id| format!("P{id}")))
                    .width(104.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut scope, None, "All programs");
                        for program in &report.programs {
                            ui.selectable_value(
                                &mut scope,
                                Some(program.id),
                                format!("P{} · {:?}", program.id, program.kind),
                            )
                            .on_hover_text(format!(
                                "{}\n{}", program.code_address, program.code_hash
                            ));
                        }
                    });
            }
            let selected_group = selection
                .state
                .and_then(|state| self.display.state_to_node.get(&state).copied());
            let multiple = selected_group
                .and_then(|id| self.node_positions.get(&id))
                .and_then(|position| self.display.nodes.get(*position))
                .is_some_and(|node| node.members.len() > 1);
            if ui
                .add_enabled(multiple, egui::Button::new("Instances").small())
                .clicked()
            {
                self.instance_group = selected_group;
                self.instance_filter.clear();
            }
            if let Some(state) = selection.state {
                ui.label(
                    RichText::new(format!(
                        "Selected S{state}{}",
                        if selected_visible { "" } else { " · outside view" }
                    ))
                    .small()
                    .color(palette::MUTED),
                );
            }
        });
        self.set_mode(mode);
        if scope != self.program_scope {
            self.program_scope = scope;
            self.invalidate_scene();
        }
        if hops != self.neighborhood_hops {
            self.neighborhood_hops = hops;
            self.invalidate_scene();
        }
    }

    /// The new projection must be ready before reserving this row, so its
    /// first fit uses the same canvas bounds as subsequent frames.
    pub(super) fn view_legend(&self, ui: &mut Ui) {
        if self.display.hidden_states > 0 || self.display.boundary_edges > 0 {
            ui.label(
                RichText::new(format!(
                    "View hides {} states · {} boundary edges",
                    self.display.hidden_states, self.display.boundary_edges
                ))
                .small()
                .color(palette::MUTED),
            )
            .on_hover_text("This is a display subset. The analysis result and its completion status are unchanged.");
        }
    }

    pub(super) fn instances(
        &mut self,
        ctx: &Context,
        report: &AnalysisReport,
        selection: &mut Selection,
    ) {
        let Some(group_id) = self.instance_group else {
            return;
        };
        let Some(node) = self
            .node_positions
            .get(&group_id)
            .and_then(|position| self.display.nodes.get(*position))
        else {
            self.instance_group = None;
            return;
        };
        let Some(index) = &self.index else {
            return;
        };
        let mut open = true;
        let mut picked = None;
        egui::Window::new("State instances")
            .id(egui::Id::new("cfg_instances"))
            .open(&mut open)
            .default_width(460.0)
            .max_width((ctx.content_rect().width() - 24.0).max(180.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "{}:B{} · {} state instances",
                    node.program.map_or("Native".into(), |id| format!("P{id}")),
                    node.basic_block,
                    node.members.len()
                ));
                ui.label(
                    RichText::new("Choose an original state to view its SSA, stack and effects. This overview does not merge their execution contexts.")
                        .small()
                        .color(palette::MUTED),
                );
                ui.horizontal(|ui| {
                    ui.label("Find state");
                    crate::framework::line_editor(
                        ui,
                        "cfg_instance_search",
                        &mut self.instance_filter,
                        "S128 or state ID",
                    );
                });
                let query = self.instance_filter.trim();
                let selected_id = query
                    .strip_prefix('S')
                    .or_else(|| query.strip_prefix('s'))
                    .unwrap_or(query)
                    .parse::<usize>()
                    .ok();
                let matching = selected_id
                    .filter(|state| self.display.state_to_node.get(state) == Some(&group_id));
                let rows = if query.is_empty() {
                    node.members.len()
                } else {
                    usize::from(matching.is_some())
                };
                if rows == 0 {
                    ui.label("No matching state in this block");
                }
                egui::ScrollArea::vertical()
                    .id_salt("cfg_instance_rows")
                    .max_height((ctx.content_rect().height() * 0.45).min(360.0))
                    .show_rows(ui, 22.0, rows, |ui, range| {
                        for row in range {
                            let state = matching.unwrap_or_else(|| node.members[row]);
                            let Some(block) = index.cfg(report, state) else {
                                continue;
                            };
                            let coverage = index.ssa(report, state).map_or(
                                evm_abstract_protocol::BlockCoverage::Unexecuted,
                                |block| block.coverage,
                            );
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            let label = format!(
                                "S{state} · {coverage:?} · frame {} · stack {} · {} jumps",
                                block.frame_depth, block.entry_stack.len(), block.context.len()
                            );
                            let tooltip = format!(
                                "Code: {}\nStorage: {}\nContext: {:?}",
                                block.code_address, block.storage_address, block.context
                            );
                            if ui
                                .selectable_label(selection.state == Some(state), label)
                                .on_hover_text(tooltip)
                                .clicked()
                            {
                                picked = Some(state);
                            }
                        }
                    });
            });
        if let Some(state) = picked {
            *selection = Selection {
                state: Some(state),
                pc: None,
            };
            open = false;
            ctx.request_repaint();
        }
        if !open {
            self.instance_group = None;
        }
    }
}
