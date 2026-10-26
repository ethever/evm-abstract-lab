//! Report-local SSA identities and facts retain their independent scopes.
use crate::report::ReportView;

use egui::{RichText, Ui};
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{
    SsaFactScope, SsaFactStatus, SsaInstruction, SsaValue, SsaValueFacts, SsaValueOrigin,
};

use super::{Selection, scroll, value};
use crate::{notation, palette, ssa_display};

pub(super) fn summary(report: &ReportView<'_>, selection: Selection) -> String {
    let Some(id) = selection.value else {
        return "Inspector: SSA value · no value selected".into();
    };
    let Some(value) = report.ssa.values.get(id) else {
        return format!("Inspector: SSA value %{id} · unavailable");
    };
    let facts = value
        .facts
        .iter()
        .map(|facts| format!("{:?} · {}", facts.status, scope(&facts.scope)))
        .collect::<Vec<_>>()
        .join("; ");
    let operation = instruction(report, value)
        .map(|instruction| ssa_display::operation_text(&instruction.operation))
        .map_or(String::new(), |operation| format!(" · {operation}"));
    format!(
        "Inspector: SSA value %{id} · {facts} · {}{operation}",
        origin(&value.origin),
    )
}

pub(super) fn show(ui: &mut Ui, report: &ReportView<'_>, selection: &mut Selection) {
    if report.ssa.values.is_empty() {
        ui.label("No SSA value definitions are available in this report.");
        return;
    }
    let mut id = selection.value.unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label("SSA value");
        let response = ui.add(
            egui::DragValue::new(&mut id)
                .prefix("%")
                .range(0..=report.ssa.values.len() - 1)
                .speed(1),
        );
        crate::controls::observe(ui, &response, "inspector_ssa_value", "text", None);
        response.clone().on_hover_text(
            "Enter a final ValueId, or click a %value in SSA. Value IDs belong to this report.",
        );
        if response.changed() {
            selection.value = Some(id);
        }
        ui.label(RichText::new("Definition and scoped facts").color(palette::MUTED));
    });
    ui.separator();
    let Some(value) = report.ssa.values.get(id) else {
        ui.colored_label(palette::WARNING, format!("SSA value %{id} is unavailable."));
        return;
    };
    scroll(ui, "ssa_value_details", |ui| {
        ui.push_id(value.id, |ui| details(ui, report, value, selection));
    });
}

fn details(ui: &mut Ui, report: &ReportView<'_>, selected: &SsaValue, selection: &mut Selection) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Value %{}", selected.id)).strong());
        if crate::controls::action(ui, "Go to source", crate::controls::Density::Compact).clicked()
        {
            *selection = source_selection(&selected.origin, selected.id);
        }
    });
    let native = instruction(report, selected);
    if let Some(instruction) = native {
        let computation = format!(
            "%{} = {}",
            selected.id,
            ssa_display::operation_text(&instruction.operation),
        );
        ui.add(egui::Label::new(RichText::new(&computation).monospace()).truncate())
            .on_hover_text(computation);
    }
    // Definition proofs lead the summary, while every recorded scope keeps
    // its own numeric details and original report-local UI identity below.
    let mut facts: Vec<_> = selected.facts.iter().enumerate().collect();
    facts.sort_by_key(|(_, fact)| {
        (
            fact.status != SsaFactStatus::Current,
            !matches!(&fact.scope, SsaFactScope::Definition),
        )
    });
    if selected.facts.is_empty() {
        ui.label("No numeric facts were collected for this definition.");
    }
    for (_, facts) in &facts {
        let numeric = facts.constant.as_ref().map_or_else(
            || {
                facts
                    .value
                    .as_ref()
                    .map_or(String::new(), |value| format!(" · {}", value.summary))
            },
            |constant| format!(" · exact {constant}"),
        );
        let summary = format!("{} · {:?}{numeric}", scope(&facts.scope), facts.status);
        ui.add(
            egui::Label::new(notation::widget(
                ui,
                RichText::new(&summary)
                    .strong()
                    .color(if facts.status == SsaFactStatus::Current {
                        palette::TEXT
                    } else {
                        palette::WARNING
                    }),
            ))
            .truncate(),
        )
        .on_hover_text(notation::widget(ui, summary));
    }
    ui.separator();
    for (index, facts) in facts {
        ui.push_id(index, |ui| fact_details(ui, facts));
        ui.separator();
    }
    ui.collapsing("Source and derivation", |ui| {
        egui::Grid::new("ssa_value_identity")
            .num_columns(2)
            .show(ui, |ui| {
                value::text(ui, "Native definition", origin(&selected.origin));
                value::text(
                    ui,
                    "Value ID before renumbering",
                    format!("%{}", selected.original_value),
                );
                value::text(
                    ui,
                    "Transformation rule",
                    format!("{:?}", selected.transformation),
                );
                if let Some(instruction) = native {
                    value::text(ui, "Native opcode", ssa_display::source_text(instruction));
                    value::text(ui, "Observed phase", format!("{:?}", instruction.progress));
                    value::text(ui, "Fault", instruction.fault.to_string());
                }
            });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Equal value").color(palette::MUTED));
            if let Some(equal) = selected.equivalent {
                value_link(ui, equal, selection);
            } else {
                ui.label("None recorded");
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Derivation inputs").color(palette::MUTED));
            if selected.dependencies.is_empty() {
                ui.label("None");
            }
            for dependency in &selected.dependencies {
                value_link(ui, *dependency, selection);
            }
        });
        ui.label(
            RichText::new(
                "Derivation inputs record dependencies; equality is recorded separately.",
            )
            .small()
            .color(palette::MUTED),
        );
    });
}

fn value_link(ui: &mut Ui, id: usize, selection: &mut Selection) {
    let response = ui.link(format!("%{id}"));
    if response.clicked() {
        selection.value = Some(id);
    }
}

fn fact_details(ui: &mut Ui, facts: &SsaValueFacts) {
    ui.label(notation::widget(
        ui,
        RichText::new(format!("{} · {:?}", scope(&facts.scope), facts.status))
            .strong()
            .color(if facts.status == SsaFactStatus::Current {
                palette::TEXT
            } else {
                palette::WARNING
            }),
    ));
    egui::Grid::new("ssa_fact_metadata")
        .num_columns(2)
        .show(ui, |ui| {
            value::text(ui, "Fact source", format!("{:?}", facts.source));
            if let Some(constant) = &facts.constant {
                value::text(ui, "Exact constant", constant);
            }
        });
    let explanation = match &facts.scope {
        SsaFactScope::Definition => {
            "Applies to this definition within this report's inputs.".into()
        }
        SsaFactScope::State(state) => format!(
            "Applies within the recorded native-state context {}.",
            Symbol::State(*state),
        ),
        SsaFactScope::Edge(edge) => format!(
            "Applies only on the recorded native transition {}.",
            Symbol::Edge(*edge),
        ),
    };
    ui.label(notation::widget(ui, RichText::new(explanation).small()));
    match facts.status {
        SsaFactStatus::Current => {}
        SsaFactStatus::Uncollected => {
            ui.colored_label(
                palette::WARNING,
                "No numeric fact was collected for this scope.",
            );
        }
        SsaFactStatus::Stale => {
            ui.colored_label(
                palette::WARNING,
                "Execution receipt is stale; this fact is not used to justify normalization.",
            );
        }
        SsaFactStatus::Unexecuted => {
            ui.colored_label(palette::WARNING, "This state has no execution receipt.");
        }
    }
    if let Some(properties) = &facts.value {
        value::details(ui, properties);
    }
}

fn scope(scope: &SsaFactScope) -> String {
    match scope {
        SsaFactScope::Definition => "Definition".into(),
        SsaFactScope::State(state) => format!("State {}", Symbol::State(*state)),
        SsaFactScope::Edge(edge) => format!("Edge {}", Symbol::Edge(*edge)),
    }
}

fn origin(origin: &SsaValueOrigin) -> String {
    match origin {
        SsaValueOrigin::Phi(origin) => format!(
            "Phi {} · {} / slot {}",
            Symbol::State(origin.state),
            Symbol::Frame(origin.frame),
            origin.slot,
        ),
        SsaValueOrigin::Instruction(origin) => format!(
            "Instruction {} · pc 0x{:x} · receipt {}",
            Symbol::State(origin.state),
            origin.pc,
            origin.instruction_index,
        ),
        SsaValueOrigin::Transition(origin) => format!(
            "Transition {} · {} → {}",
            Symbol::Edge(origin.edge),
            Symbol::State(origin.from),
            Symbol::State(origin.to),
        ),
    }
}

fn source_selection(origin: &SsaValueOrigin, value: usize) -> Selection {
    match origin {
        SsaValueOrigin::Phi(origin) => Selection {
            state: Some(origin.state),
            pc: None,
            value: Some(value),
        },
        SsaValueOrigin::Instruction(origin) => Selection {
            state: Some(origin.state),
            pc: Some(origin.pc),
            value: Some(value),
        },
        SsaValueOrigin::Transition(origin) => Selection {
            state: Some(origin.to),
            pc: None,
            value: Some(value),
        },
    }
}

fn instruction<'a>(report: &'a ReportView<'_>, value: &SsaValue) -> Option<&'a SsaInstruction> {
    let SsaValueOrigin::Instruction(origin) = &value.origin else {
        return None;
    };
    report
        .ssa
        .blocks
        .iter()
        .find(|block| block.state == origin.state)?
        .instructions
        .get(origin.instruction_index)
        .filter(|instruction| instruction.pc == origin.pc)
}

#[cfg(test)]
mod tests;
