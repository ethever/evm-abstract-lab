//! Virtualized choices preserve each call/receipt association independently.
use super::{Graph, GraphIntent, control::Destination, scene::Picker};
use egui::{Rect, Ui};
use evm_abstract_protocol::{AnalysisReport, ProcessLocation};

impl Graph {
    pub(super) fn minimum_picker(
        &mut self,
        ui: &Ui,
        report: &AnalysisReport,
        canvas: Rect,
    ) -> Option<GraphIntent> {
        let scene = self.scene.minimum.as_mut()?;
        let picker = scene.picker?;
        let rows = match picker {
            Picker::Origins(id) => scene.control.cards[id].origins.len(),
            Picker::Targets(id) => scene.control.target_sets[id].entries.len(),
        };
        let mut open = true;
        let mut picked = None;
        egui::Window::new(match picker { Picker::Origins(_) => "Control receipts", Picker::Targets(_) => "Control destinations" }).id(ui.id().with("control_destinations"))
            .constrain_to(canvas).default_pos(canvas.left_top() + egui::Vec2::splat(8.0))
            .open(&mut open).default_width(460.0).max_width((ui.ctx().content_rect().width() - 24.0).max(180.0))
            .show(ui.ctx(), |ui| {
                ui.label(match picker { Picker::Origins(_) => "Select a retained receipt. Each row keeps its own currentness and state evidence.", Picker::Targets(_) => "Open a retained destination or decoded source candidate. Source candidates carry no receipt or Native state." });
                egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() * 0.5).min(360.0))
                    .show_rows(ui, 24.0, rows, |ui, range| {
                        for row in range {
                            let (destination, details) = match picker {
                                Picker::Origins(id) => { let origin = &scene.control.cards[id].origins[row];
                                    (Destination::Control(origin.target), format!("Current {} · complete {} · excluded {} · boundary {} · Body transferred {} · input {}", origin.current, origin.complete, origin.excluded, origin.boundary, origin.transferred, origin.input_revision)) },
                                Picker::Targets(id) => { let entry = &scene.control.target_sets[id].entries[row]; (entry.destination, entry.via.detail(report, scene.scope.summary())) },
                            };
                            let mut label = destination_label(report, scene.scope.summary(), destination);
                            if let (Picker::Origins(_), Destination::Control(target)) = (picker, destination) { label = format!("r{} {} · {label}", target.origin.node, if target.origin.location.is_none() { "Body" } else { "MAY point" }); }
                            let response = ui.add(egui::Button::new(&label).truncate()).on_hover_text(details);
                            crate::controls::observe(ui, &response, &label, match picker {
                                Picker::Origins(_) => "graph-origin",
                                Picker::Targets(_) => "graph-destination",
                            }, None);
                            if response.clicked() {
                                picked = Some(match (picker, destination) { (Picker::Origins(_), Destination::Control(target)) => GraphIntent::Select(target), _ => destination_intent(scene.scope.summary(), destination) });
                            }
                        }
                    });
            });
        if !open || picked.is_some() {
            scene.picker = None;
        }
        picked
    }
}

pub(super) fn destination_intent(summary: usize, destination: Destination) -> GraphIntent {
    match destination {
        Destination::Control(target) => GraphIntent::Open(target),
        Destination::Source { program, location } => GraphIntent::OpenSource {
            summary,
            program,
            location,
        },
    }
}

fn destination_label(report: &AnalysisReport, summary: usize, destination: Destination) -> String {
    match destination {
        Destination::Source { program, location } => {
            format!(
                "Decoded source · {}",
                source_label(report, Some(program), Some(location))
            )
        }
        Destination::Control(target) => {
            let node = report
                .minimum
                .as_ref()
                .and_then(|model| model.summaries.get(summary))
                .and_then(|summary| {
                    summary
                        .relation
                        .regional_cfg
                        .nodes
                        .iter()
                        .find(|node| node.id == target.origin.node)
                });
            let frame = node.and_then(|node| node.frame.process.as_ref());
            let program = frame
                .and_then(|frame| report.process_program(&frame.source))
                .map(|program| program.id);
            let location = target
                .origin
                .location
                .or_else(|| frame.map(|frame| frame.location));
            let qualifier = frame.map_or(String::new(), |frame| {
                format!(" · {:?} · static {}", frame.owner, frame.is_static)
            });
            format!("{}{qualifier}", source_label(report, program, location))
        }
    }
}

fn source_label(
    report: &AnalysisReport,
    program: Option<usize>,
    location: Option<ProcessLocation>,
) -> String {
    let source = program.and_then(|id| report.programs.iter().find(|program| program.id == id));
    let label = match location {
        Some(ProcessLocation::Block(index)) => source
            .and_then(|source| source.blocks.get(index))
            .map_or_else(
                || "Uncaptured source block".into(),
                |block| {
                    format!(
                        "{} · {}",
                        crate::widgets::graph::source::title(block.start_pc, &block.instructions),
                        crate::widgets::graph::source::ending(&block.instructions)
                    )
                },
            ),
        Some(ProcessLocation::JumpTargets) => "Unknown jump targets".into(),
        Some(ProcessLocation::Any) => "Unknown code".into(),
        Some(ProcessLocation::End) => "End of code".into(),
        None => "Control receipt".into(),
    };
    let address = source.and_then(|program| program.code_address.as_deref());
    address.map_or(label.clone(), |address| {
        format!(
            "{} · {label}",
            if address.len() > 18 && address.is_ascii() {
                format!("{}…{}", &address[..8], &address[address.len() - 6..])
            } else {
                address.into()
            }
        )
    })
}
