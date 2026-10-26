//! Flat, virtualized inspection of retained relationships. Exact payloads and
//! logical ranges are summarized without recursive rendering or byte expansion.
use super::value;
use crate::report::ReportView;
use crate::{notation, palette};
use egui::{RichText, Ui};
use evm_abstract_notation::WideSymbol;
use evm_abstract_protocol as api;
use std::collections::BTreeSet;

fn e(reference: api::ExpressionRef) -> String {
    WideSymbol::Expression(reference.node as u64).to_string()
}
fn condition(atom: &api::PathConstraint) -> String {
    use api::PathConstraint as C;
    match atom {
        C::Truth(x) => format!(
            "{} {} 0",
            e(x.expression),
            if x.nonzero { "≠" } else { "=" }
        ),
        C::Range(x) => format!("{} ≤u {} ≤u {}", x.lower, e(x.expression), x.upper),
        C::SignedRange(x) => format!(
            "{} ≤ ({} xor 2^255) ≤ {}",
            x.lower,
            e(x.expression),
            x.upper
        ),
        C::Modulo(x) => format!("{} = {} mod {}", e(x.expression), x.residue, x.modulus),
        C::Bits(x) => format!("{}: zero {}, one {}", e(x.expression), x.zero, x.one),
        C::Equal(x) => format!("{} = {}", e(x.left), e(x.right)),
        C::Members(x) => format!(
            "{} ∈ {{{}}} ({} values)",
            e(x.expression),
            x.values
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            x.values.len()
        ),
    }
}
pub(super) fn label(node: &api::Expression) -> String {
    use api::Expression as N;
    match node {
        N::Constant(word) => word.clone(),
        N::Input(input) => format!(
            "input {} · {}",
            WideSymbol::Scope(input.scope.0),
            value::symbol(&input.symbol)
        ),
        N::Fresh(id) => WideSymbol::Fresh(*id).to_string(),
        N::Operation(op) => format!(
            "opcode 0x{:02x}({})",
            op.opcode,
            op.arguments
                .iter()
                .copied()
                .map(e)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        N::Ite(x) => format!(
            "if {} ≠ 0 then {} else {}",
            e(x.condition),
            e(x.then_value),
            e(x.else_value)
        ),
        N::ByteRead(x) => format!("byte read bytes#{}[{}]", x.state.node, e(x.index)),
        N::WordRead(x) => format!(
            "word read view#{} at {} · big endian, zero padded",
            x.view.node,
            e(x.offset)
        ),
        N::SlotRead(x) => format!(
            "slot read slots#{}[{}] · alias depends on path facts",
            x.state.node,
            e(x.key)
        ),
        N::Keccak(x) => format!("keccak(view#{}) · same content gives same hash", x.node),
        N::BytesZero => "zero bytes".into(),
        N::BytesInput(x) => format!(
            "byte source {} · namespace {} · length {} · {} observations",
            e(x.source.token),
            x.source.namespace,
            e(x.length),
            x.bindings.len()
        ),
        N::BytesExact(bytes) => format!(
            "exact source · {} bytes · {}{}",
            bytes.len().saturating_sub(2) / 2,
            bytes.chars().take(130).collect::<String>(),
            if bytes.len() > 130 { "…" } else { "" }
        ),
        N::WriteByte(x) => format!(
            "write byte bytes#{}[{}] ← {}",
            x.previous.node,
            e(x.offset),
            e(x.value)
        ),
        N::WriteWord(x) => format!(
            "write word bytes#{}[{}..+32] ← {}",
            x.previous.node,
            e(x.offset),
            e(x.value)
        ),
        N::Copy(x) => format!(
            "copy bytes#{}[{}..+{}] ← view#{} · captured source",
            x.previous.node,
            e(x.destination),
            e(x.length),
            x.source.node
        ),
        N::Slice(x) => format!("slice view#{} · bounded zero padding", x.node),
        N::BytesIte(x) => format!(
            "if {} ≠ 0 then bytes#{} else bytes#{}",
            e(x.condition),
            x.then_state.node,
            x.else_state.node
        ),
        N::SlotsZero => "zero slots".into(),
        N::SlotsInput(x) => format!(
            "slot source {} · account/plane namespace {} · {} observations",
            e(x.source.token),
            x.source.namespace,
            x.bindings.len()
        ),
        N::Store(x) => format!(
            "store slots#{}[{}] ← {} · ordered update",
            x.previous.node,
            e(x.key),
            e(x.value)
        ),
        N::SlotsIte(x) => format!(
            "if {} ≠ 0 then slots#{} else slots#{}",
            e(x.condition),
            x.then_state.node,
            x.else_state.node
        ),
        N::View(x) => format!(
            "view bytes#{} offset {} length {} · zero padded",
            x.state.node,
            e(x.offset),
            e(x.length)
        ),
        N::PathTrue => "path true".into(),
        N::PathFalse => "path false · proved impossible".into(),
        N::PathAtoms(atoms) => format!(
            "path AND ({} guarantees): {}{}",
            atoms.len(),
            atoms
                .iter()
                .take(4)
                .map(condition)
                .collect::<Vec<_>>()
                .join("; "),
            if atoms.len() > 4 { "; …" } else { "" }
        ),
        N::PathAnd(paths) => format!(
            "path AND ({})",
            paths
                .iter()
                .take(8)
                .map(|p| format!("path#{}", p.node))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        N::PathIte(x) => format!(
            "if {} ≠ 0 then path#{} else path#{}",
            e(x.selector),
            x.then_path.node,
            x.else_path.node
        ),
    }
}
fn reachable(graph: &api::SymbolicGraph, root: usize) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        if !seen.insert(index) {
            continue;
        }
        if let Some(node) = graph.nodes.get(index) {
            pending.extend(node.references().into_iter().map(|r| r.node));
        }
    }
    seen.into_iter().collect()
}
pub(super) fn show(ui: &mut Ui, report: &ReportView<'_>, root: usize, title: &str) {
    egui::CollapsingHeader::new(format!("{title} · root #{root}"))
        .id_salt(("symbolic_root",title,root))
        .show(ui,|ui| {
            let rows=reachable(&report.symbolic,root);
            ui.label(RichText::new(format!("{} shared nodes · retained relations; read numeric and path facts for proved conclusions",rows.len())).small().color(palette::MUTED));
            egui::ScrollArea::both().id_salt(("symbolic_rows",title,root)).max_height(240.0)
                .show_rows(ui,21.0,rows.len(),|ui,range| {
                    for row in range {
                        let index=rows[row];
                        if let Some(node)=report.symbolic.nodes.get(index) {
                            let text=format!("#{index} = {}",label(node));
                            ui.add(egui::Label::new(notation::widget(ui,RichText::new(&text).monospace())).truncate()).on_hover_text(notation::widget(ui,&text));
                        } else {ui.colored_label(palette::ERROR,"Unavailable symbolic node");}
                    }
                });
        });
}
pub(super) fn facts(ui: &mut Ui, facts: &[api::SymbolicFact], title: &str) {
    if facts.is_empty() {
        return;
    }
    ui.collapsing(format!("{title} · {} expressions", facts.len()), |ui| {
        egui::ScrollArea::both()
            .id_salt(("symbolic_facts", title))
            .max_height(160.0)
            .show_rows(ui, 21.0, facts.len(), |ui, range| {
                for index in range {
                    let fact = &facts[index];
                    ui.horizontal(|ui| {
                        ui.monospace(e(fact.expression));
                        value::cell(ui, &fact.value);
                    });
                }
            });
    });
}
pub(super) fn pending(ui: &mut Ui, report: &ReportView<'_>, id: usize) {
    let Some(machine) = report.pending_machines.get(id) else {
        ui.colored_label(palette::ERROR, "Unavailable pending state");
        return;
    };
    ui.collapsing(
        format!(
            "Pending state #{id} · {} frames · store #{}",
            machine.frames.len(),
            machine.store
        ),
        |ui| {
            show(ui, report, machine.path.node, "Paired path guards");
            for frame in &machine.frames {
                ui.collapsing(
                    format!(
                        "Frame {} · storage {} · code {}",
                        frame.index,
                        frame
                            .storage_address
                            .as_deref()
                            .unwrap_or("Process owner class"),
                        frame.code_address.as_deref().unwrap_or("Process source")
                    ),
                    |ui| {
                        if frame.process.is_some() {
                            super::context::frame(ui, report, frame);
                            return;
                        }
                        super::buffers::stack(ui, &frame.stack);
                        if let Some(id) = frame.memory {
                            super::buffers::show(ui, report, id, "Pre-effect memory");
                        }
                        if let Some(id) = frame.calldata {
                            super::buffers::show(ui, report, id, "Captured calldata");
                        }
                        if let Some(store) = report.stores.get(machine.store) {
                            for transient in [false, true] {
                                super::stores::storage(
                                    ui,
                                    report,
                                    Some(store),
                                    frame.storage_address.as_deref().unwrap_or("Not recorded"),
                                    transient,
                                );
                            }
                        }
                    },
                );
            }
        },
    );
}
#[cfg(test)]
mod tests;
