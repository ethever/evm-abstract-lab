//! Regional flow has its own identities and granularity, separate from native SSA.
use egui::Ui;
use evm_abstract_protocol::{CodeKind, RegionalCfg, RegionalNode, SummarySourceStatus};

pub(super) fn status(node: &RegionalNode, source: SummarySourceStatus) -> &'static str {
    match source {
        SummarySourceStatus::Historical => return "Historical observation",
        SummarySourceStatus::Uncertified => return "Final source uncertified",
        SummarySourceStatus::Current => {}
    }
    if node.excluded {
        return "Excluded input";
    }
    match &node.transfer {
        None => "Admitted; transfer pending",
        Some(transfer) if !transfer.current || transfer.input_revision != node.input_revision => {
            "Historical transfer"
        }
        Some(transfer) if !transfer.complete => "Current partial transfer",
        Some(transfer) if transfer.visited_block => "Current block transfer",
        Some(_) if node.frame.kind == CodeKind::AbstractEnvironment => {
            "Current effect/environment abstraction"
        }
        Some(_) => "Current transfer; no captured block visit",
    }
}

pub(super) fn show(ui: &mut Ui, cfg: &RegionalCfg, source: SummarySourceStatus) {
    ui.collapsing("Regional control flow", |ui| {
        ui.label("Summarized flow uses regional node IDs; instruction-level SSA is separate.");
        ui.label(format!(
            "{} nodes · {} calls",
            cfg.nodes.len(),
            cfg.calls.len()
        ));
        for node in &cfg.nodes {
            ui.push_id(("regional_node", node.id), |ui| {
                ui.collapsing(
                    format!(
                        "r{} · procedure {} · {}",
                        node.id,
                        node.procedure,
                        status(node, source)
                    ),
                    |ui| node_details(ui, node, source),
                );
            });
        }
        ui.collapsing("Calls and returns", |ui| {
            for call in &cfg.calls {
                ui.push_id(("regional_call", call.id), |ui| {
                    let local_current = cfg.nodes.get(call.from).is_some_and(|node| {
                        !node.excluded
                            && node.transfer.as_ref().is_some_and(|transfer| {
                                transfer.current
                                    && transfer.input_revision == node.input_revision
                                    && transfer.calls.contains(&call.id)
                            })
                    });
                    let call_scope = match source {
                        SummarySourceStatus::Historical => "historical observation",
                        SummarySourceStatus::Uncertified => "final source uncertified",
                        SummarySourceStatus::Current if local_current => "current for final source",
                        SummarySourceStatus::Current => "record observation",
                    };
                    ui.label(format!(
                        "Call c{} · {call_scope}: r{} → procedure {} · entry nodes {:?}",
                        call.id, call.from, call.callee, call.entries
                    ));
                    ui.label(format!(
                        "Observed caller input revision: {}",
                        call.input_revision.0
                    ));
                    for delivery in &call.deliveries {
                        let local_current =
                            delivery.current && delivery.input_revision == call.input_revision;
                        let label = match source {
                            SummarySourceStatus::Historical => "Historical observation delivery",
                            SummarySourceStatus::Uncertified => "Uncertified source delivery",
                            SummarySourceStatus::Current if local_current => "Current delivery",
                            SummarySourceStatus::Current => "Historical delivery",
                        };
                        ui.label(format!("Record-local delivery current: {local_current}"));
                        ui.label(format!(
                            "{label} · {:?} · input revision {} · output generation {}",
                            delivery.kind, delivery.input_revision.0, delivery.output_generation
                        ));
                        for edge in &delivery.successors {
                            ui.monospace(format!(
                                "{label}: r{} → r{} · {:?}",
                                call.from, edge.to, edge.kind
                            ));
                        }
                    }
                });
            }
        });
    });
}

fn node_details(ui: &mut Ui, node: &RegionalNode, source: SummarySourceStatus) {
    let frame = &node.frame;
    ui.monospace(format!("Code: {}", frame.code_address));
    if let Some(hash) = &frame.code_hash {
        ui.monospace(format!("Captured code hash: {hash}"));
    }
    ui.monospace(format!("Storage namespace: {}", frame.storage_address));
    ui.label(format!(
        "ADDRESS: {:?} · CALLER: {:?}",
        frame.address_value, frame.caller
    ));
    ui.label(format!(
        "Mode: {:?} · static: {}",
        frame.kind, frame.is_static
    ));
    if let Some(block) = frame.basic_block {
        ui.label(format!(
            "Program block: {block} · stack height: {} · jump context: {:?}",
            frame.stack_height, frame.context
        ));
    }
    if frame.kind == CodeKind::AbstractEnvironment {
        ui.label("Effect/environment abstraction: callbacks may be represented by effects even when the retained call list is empty.");
    }
    ui.label(format!(
        "Observed input revision: {}",
        node.input_revision.0
    ));
    ui.label(format!("Excluded input: {}", node.excluded));
    ui.label(format!("Admitted successors: {:?}", node.successors));
    if let Some(transfer) = &node.transfer {
        let local_current =
            transfer.current && transfer.input_revision == node.input_revision && !node.excluded;
        let label = match source {
            SummarySourceStatus::Historical => "Historical observation transfer",
            SummarySourceStatus::Uncertified => "Uncertified source transfer",
            SummarySourceStatus::Current if local_current => "Current transfer",
            SummarySourceStatus::Current => "Historical transfer",
        };
        ui.label(format!("Record-local transfer current: {local_current}"));
        ui.label(format!(
            "{label} · input revision {} · complete: {} · captured block visited: {}",
            transfer.input_revision.0, transfer.complete, transfer.visited_block
        ));
        for edge in &transfer.successors {
            ui.monospace(format!(
                "{label}: r{} → r{} · {:?}",
                node.id, edge.to, edge.kind
            ));
        }
        ui.label(format!(
            "Retained calls: {:?} · emitted results: {:?}",
            transfer.calls, transfer.outcomes
        ));
    } else {
        ui.label("Admitted; transfer pending");
    }
}
