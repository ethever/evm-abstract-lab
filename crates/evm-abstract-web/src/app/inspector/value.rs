//! Readable numeric/symbolic components, without reducing domain evidence to JSON.

use crate::palette;
use egui::{RichText, Ui};
use evm_abstract_protocol::{AddressValue, CongruenceValue, Expression, InputSymbol, ValueInfo};

pub(super) fn address(value: &AddressValue) -> String {
    match value {
        AddressValue::Concrete(address) => address.clone(),
        AddressValue::Symbolic(input) => format!("symbolic {}", symbol(input)),
    }
}

fn symbol(value: &InputSymbol) -> String {
    value.index.as_ref().map_or_else(
        || format!("{:?}", value.kind),
        |index| format!("{:?}[{index}]", value.kind),
    )
}

pub(super) fn cell(ui: &mut Ui, value: &ValueInfo) {
    ui.add(egui::Label::new(RichText::new(&value.summary).monospace()).truncate())
        .on_hover_ui(|ui| {
            ui.set_max_width(600.0);
            egui::ScrollArea::both()
                .max_height(320.0)
                .show(ui, |ui| details(ui, value));
        });
}

pub(super) fn named(ui: &mut Ui, label: &str, value: &ValueInfo) {
    ui.label(RichText::new(label).color(palette::MUTED));
    cell(ui, value);
    ui.end_row();
}

pub(super) fn text(ui: &mut Ui, label: &str, value: impl Into<String>) {
    let value = value.into();
    ui.label(RichText::new(label).color(palette::MUTED));
    ui.add(egui::Label::new(RichText::new(&value).monospace()).truncate())
        .on_hover_text(&value);
    ui.end_row();
}

pub(super) fn details(ui: &mut Ui, value: &ValueInfo) {
    ui.label(RichText::new(&value.summary).monospace().strong());
    egui::Grid::new("abstract_value_components")
        .num_columns(2)
        .show(ui, |ui| {
            text(
                ui,
                "Constants",
                match &value.constants {
                    None => "Unrestricted".into(),
                    Some(values) if values.is_empty() => "∅".into(),
                    Some(values) => values.join(", "),
                },
            );
            text(ui, "Known zero bits", &value.known_zero);
            text(ui, "Known one bits", &value.known_one);
            text(
                ui,
                "Unsigned range",
                format!("{} … {}", value.unsigned.lower, value.unsigned.upper),
            );
            text(
                ui,
                "Signed range (bit patterns)",
                format!("{} … {}", value.signed.lower, value.signed.upper),
            );
            text(
                ui,
                "Congruence",
                match &value.congruence {
                    CongruenceValue::Any => "Unrestricted".into(),
                    CongruenceValue::Exact(word) => word.clone(),
                    CongruenceValue::Modulo(class) => {
                        format!("{} mod {}", class.residue, class.modulus)
                    }
                },
            );
            text(
                ui,
                "Possible origins",
                value.origins.as_ref().map_or_else(
                    || "Unknown".into(),
                    |origins| {
                        origins
                            .iter()
                            .map(|origin| format!("{origin:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                ),
            );
            if let Some(identity) = &value.identity {
                text(
                    ui,
                    "Equality identity",
                    format!("scope {} · {}", identity.scope, symbol(&identity.symbol)),
                );
            }
            if value.code_address_role {
                text(ui, "Role", "Code address");
            }
        });
    if value.symbolic_limit {
        ui.colored_label(palette::WARNING, "Symbolic retention limit reached");
    }
    if let Some(graph) = &value.expression {
        ui.separator();
        ui.label(format!("Expression root #{}", graph.root));
        for (index, expression) in graph.nodes.iter().enumerate() {
            let expression = match expression {
                Expression::Constant(word) => word.clone(),
                Expression::Input(input) => {
                    format!("input {} · {}", input.scope, symbol(&input.symbol))
                }
                Expression::Fresh(id) => format!("fresh {id}"),
                Expression::Operation(operation) => format!(
                    "opcode 0x{:02x}({})",
                    operation.opcode,
                    operation
                        .arguments
                        .iter()
                        .map(|id| format!("#{id}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            ui.monospace(format!("#{index} = {expression}"));
        }
    }
}
