//! The Global view uses canonical procedure/control identities throughout rich
//! export. Its selection never contains a Native state, instruction or SSA ID.

use evm_abstract_protocol::{
    AnalysisReport, MinimumModel, MinimumSummary, ProcessSource, SummarySourceStatus,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) summary: usize,
    pub(crate) node: Option<usize>,
    pub(crate) procedure: usize,
    pub(crate) output: Option<usize>,
    pub(crate) account: Option<usize>,
}

impl Selection {
    pub(crate) fn select_summary(&mut self, model: &MinimumModel, summary: usize) {
        self.summary = summary;
        self.procedure = model
            .summaries
            .get(summary)
            .and_then(|summary| summary.relation.root_procedure)
            .unwrap_or(0);
        self.node = model
            .summaries
            .get(summary)
            .and_then(|summary| {
                summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.procedure == self.procedure)
            })
            .map(|node| node.id);
        self.output = None;
    }

    pub(crate) fn select_node(&mut self, model: &MinimumModel, id: usize) {
        if let Some(node) = model.summaries.get(self.summary).and_then(|summary| {
            summary
                .relation
                .regional_cfg
                .nodes
                .iter()
                .find(|node| node.id == id)
        }) {
            self.node = Some(node.id);
            self.procedure = node.procedure;
            self.output = None;
        }
    }

    pub(crate) fn summary<'a>(&self, model: &'a MinimumModel) -> Option<&'a MinimumSummary> {
        model.summaries.get(self.summary)
    }
}

pub(crate) fn initial_summary(model: &MinimumModel) -> usize {
    model
        .summaries
        .iter()
        .rposition(|summary| {
            summary.relation.source_current
                && summary
                    .relation
                    .root_procedure
                    .and_then(|index| summary.relation.procedures.get(index))
                    .and_then(|procedure| procedure.process.as_ref())
                    .is_some_and(|frame| frame.source == model.declared_root_source)
        })
        .or_else(|| {
            model
                .summaries
                .iter()
                .rposition(|summary| summary.relation.source_current)
        })
        .unwrap_or(0)
}

pub(crate) fn source_status(model: &MinimumModel, index: usize) -> SummarySourceStatus {
    let summary = &model.summaries[index].relation;
    summary.source_status(
        model.summaries[index + 1..]
            .iter()
            .any(|later| later.relation.source_state == summary.source_state),
    )
}

pub(super) fn source_program(report: &AnalysisReport, source: &ProcessSource) -> Option<usize> {
    report
        .programs
        .iter()
        .find(|program| {
            program.fork == source.fork()
                && program.kind == source.kind()
                && match source {
                    ProcessSource::Runtime(source) => {
                        program.code_address.as_ref() == Some(&source.source)
                            && program.code_hash == source.hash.to_string()
                    }
                    ProcessSource::InputProgram(source) => {
                        program.code_address.is_none()
                            && program.code_hash == source.hash.to_string()
                    }
                    ProcessSource::InitCode(source) => {
                        program.code_address.is_none()
                            && program.code_hash == source.hash.to_string()
                    }
                    _ => false,
                }
        })
        .map(|program| program.id)
}

#[derive(Default)]
pub(super) struct SourceCache {
    key: Option<(super::model::ReportViewId, usize)>,
    blocks: Vec<usize>,
}

impl SourceCache {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        report: &AnalysisReport,
        identity: super::model::ReportViewId,
        program: Option<usize>,
    ) {
        let Some(program) =
            program.and_then(|id| report.programs.iter().find(|program| program.id == id))
        else {
            crate::widgets::empty(ui, "No captured source for this selection");
            return;
        };
        crate::widgets::heading(
            ui,
            "DISASSEMBLY",
            &format!("Program {} · source provenance", program.id),
        );
        if self.key != Some((identity, program.id)) {
            self.blocks.clear();
            self.blocks.push(0);
            for block in &program.blocks {
                self.blocks
                    .push(self.blocks.last().copied().unwrap() + block.instructions.len() + 1);
            }
            self.key = Some((identity, program.id));
        }
        let rows = self.blocks.last().copied().unwrap_or(0);
        egui::ScrollArea::both()
            .id_salt("minimum_source_rows")
            .auto_shrink([false, false])
            .show_rows(ui, 22.0, rows, |ui, range| {
                for row in range {
                    let block = self
                        .blocks
                        .partition_point(|start| *start <= row)
                        .saturating_sub(1)
                        .min(program.blocks.len() - 1);
                    let source = &program.blocks[block];
                    let local = row - self.blocks[block];
                    if local == 0 {
                        ui.monospace(format!("B{} · 0x{:04x}", source.id, source.start_pc));
                    } else {
                        let instruction = &source.instructions[local - 1];
                        ui.label(
                            egui::RichText::new(format!(
                                "{:04x}  {}{}",
                                instruction.pc,
                                instruction.name,
                                instruction
                                    .immediate
                                    .as_ref()
                                    .map_or(String::new(), |value| format!(" {value}"))
                            ))
                            .monospace()
                            .color(crate::palette::MUTED),
                        );
                    }
                }
            });
    }
}

pub(super) fn rich_status(report: &AnalysisReport) -> String {
    match &report.rich_export {
        evm_abstract_protocol::RichExportOutcome::NotRequested => {
            "Detailed presentation pending".into()
        }
        evm_abstract_protocol::RichExportOutcome::Complete => "Detailed presentation ready".into(),
        evm_abstract_protocol::RichExportOutcome::Cancelled => {
            "Detailed presentation cancelled".into()
        }
        evm_abstract_protocol::RichExportOutcome::Refused(error) => {
            format!("Detailed presentation refused: {:?}", error.code)
        }
    }
}

impl super::Workspace {
    pub(super) fn show_minimum(&mut self, ui: &mut egui::Ui, viewport: egui::Vec2) -> bool {
        let report = self.report.as_ref().unwrap();
        let model = report.minimum.as_deref().unwrap();
        let identity = self.report_view.identity();
        let before = self.minimum_selection;
        egui::Panel::top("result_context")
            .frame(crate::palette::chrome_frame())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Global model").strong());
                    ui.label(format!("{:?} · facts {}", model.status, model.revision));
                    ui.add(egui::Label::new(rich_status(report)).truncate())
                        .on_hover_text(match &report.rich_export {
                            evm_abstract_protocol::RichExportOutcome::Refused(error) => {
                                error.message.clone()
                            }
                            _ => String::new(),
                        });
                });
                let old_summary = self.minimum_selection.summary;
                egui::ComboBox::from_id_salt("minimum_summary")
                    .selected_text(format!(
                        "Relation source {}",
                        self.minimum_selection
                            .summary(model)
                            .map_or(0, |summary| summary.relation.source_state)
                    ))
                    .show_ui(ui, |ui| {
                        for (id, summary) in model.summaries.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.minimum_selection.summary,
                                id,
                                format!(
                                    "Relation source {} · {:?} · {} procedures",
                                    summary.relation.source_state,
                                    summary.relation.kind,
                                    summary.relation.procedures.len()
                                ),
                            );
                        }
                    });
                if old_summary != self.minimum_selection.summary {
                    self.minimum_selection
                        .select_summary(model, self.minimum_selection.summary);
                }
            });
        let maximized = self.inspector.maximized();
        if self.inspector.expanded && !maximized {
            let response = self.inspector.docked_panel(ui, viewport.y).show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                self.inspector
                    .show_minimum(ui, report, model, &mut self.minimum_selection);
            });
            self.inspector.remember_resize(ui, response.response.rect);
        }
        let mut picked = None;
        if self.directory.open && !maximized {
            if viewport.x >= 1050.0 {
                egui::Panel::left("world_directory")
                    .resizable(true)
                    .default_size(196.0)
                    .frame(crate::palette::chrome_frame())
                    .show(ui, |ui| {
                        picked = directory(
                            ui,
                            report,
                            model,
                            self.program,
                            self.minimum_selection.account,
                        );
                    });
            } else {
                let mut open = true;
                egui::Window::new("Programs and accounts")
                    .id(egui::Id::new("minimum_directory_window"))
                    .open(&mut open)
                    .default_width((viewport.x - 30.0).min(360.0))
                    .max_height((viewport.y - 80.0).max(80.0))
                    .show(ui.ctx(), |ui| {
                        picked = directory(
                            ui,
                            report,
                            model,
                            self.program,
                            self.minimum_selection.account,
                        );
                    });
                self.directory.open = open && picked.is_none();
            }
        }
        if let Some(pick) = picked {
            match pick {
                Pick::Account(index) => {
                    self.minimum_selection.account = Some(index);
                    self.inspector.expanded = true;
                    self.inspector.tab = super::inspector::Tab::Frame;
                    self.program = model.accounts[index].program;
                }
                Pick::Program(id) => self.program = Some(id),
            }
        }
        let mut ready = true;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(crate::palette::BACKGROUND)
                    .inner_margin(4.0),
            )
            .show(ui, |ui| {
                if maximized {
                    self.inspector
                        .show_minimum(ui, report, model, &mut self.minimum_selection);
                    return;
                }
                if self.view == super::View::Disassembly {
                    self.minimum_source.show(ui, report, identity, self.program);
                    return;
                }
                if self.view == super::View::Split && ui.available_width() >= 900.0 {
                    egui::Panel::left("minimum_source")
                        .resizable(true)
                        .default_size(260.0)
                        .show(ui, |ui| {
                            self.minimum_source.show(ui, report, identity, self.program)
                        });
                }
                if self.view == super::View::Ssa {
                    ui.horizontal(|ui| {
                        ui.label("Instruction SSA is available in Detailed paths.");
                        if ui
                            .add_enabled(
                                report.native.is_some(),
                                egui::Button::new("Open Detailed paths"),
                            )
                            .clicked()
                        {
                            self.minimum_native_requested = true;
                        }
                    });
                }
                self.graph
                    .show_minimum(ui, model, identity, &mut self.minimum_selection);
                ready = self.graph.prepared();
            });
        if before.account != self.minimum_selection.account {
            self.program = self
                .minimum_selection
                .account
                .and_then(|index| model.accounts.get(index))
                .and_then(|account| account.program);
        } else if (before.summary, before.procedure, before.node)
            != (
                self.minimum_selection.summary,
                self.minimum_selection.procedure,
                self.minimum_selection.node,
            )
            && let Some(source) = self
                .minimum_selection
                .summary(model)
                .and_then(|summary| {
                    summary
                        .relation
                        .procedures
                        .get(self.minimum_selection.procedure)
                })
                .and_then(|procedure| procedure.process.as_ref())
                .map(|frame| &frame.source)
        {
            self.program = source_program(report, source);
        }
        if before != self.minimum_selection {
            ui.ctx().request_repaint();
        }
        ready
    }

    pub(super) fn minimum_status(&self, report: &AnalysisReport, model: &MinimumModel) -> String {
        format!(
            "{}: Global model {:?}; {} qualified relations; {} original accounts; {} programs; {}; {}; {}",
            self.task_label(),
            model.status,
            model.summaries.len(),
            model.accounts.len(),
            report.programs.len(),
            rich_status(report),
            self.graph.accessible_summary(),
            self.inspector.minimum_summary(&self.minimum_selection)
        )
    }
}

#[derive(Clone, Copy)]
enum Pick {
    Account(usize),
    Program(usize),
}

fn directory(
    ui: &mut egui::Ui,
    report: &AnalysisReport,
    model: &MinimumModel,
    program: Option<usize>,
    account: Option<usize>,
) -> Option<Pick> {
    let mut picked = None;
    ui.heading("Original input directory");
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (id, row) in model.accounts.iter().enumerate() {
            if ui
                .selectable_label(
                    account == Some(id),
                    format!(
                        "{}{}",
                        if row.root_template {
                            "Input template · "
                        } else {
                            "Account · "
                        },
                        row.address
                    ),
                )
                .clicked()
            {
                picked = Some(Pick::Account(id));
            }
        }
        ui.separator();
        ui.label("Captured programs");
        for row in &report.programs {
            if ui
                .selectable_label(
                    program == Some(row.id),
                    format!("Program {} · {:?}", row.id, row.kind),
                )
                .clicked()
            {
                picked = Some(Pick::Program(row.id));
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests;
