//! Regional flow has its own identities and granularity, separate from native SSA.
use super::process;
use egui::Ui;
use evm_abstract_protocol::{CodeKind, RegionalCfg, RegionalNode, SummarySourceStatus};

pub(super) fn status(node: &RegionalNode, source: SummarySourceStatus) -> &'static str {
    crate::report::regional_status(node, source)
}

pub(in crate::app::inspector) fn show(ui: &mut Ui, cfg: &RegionalCfg, source: SummarySourceStatus) {
    crate::framework::section(ui, "Regional control flow", |ui| {
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
        crate::framework::section(ui, "Calls and returns", |ui| {
            for call in &cfg.calls {
                ui.push_id(("regional_call", call.id), |ui| {
                    let local_current = cfg
                        .nodes
                        .iter()
                        .find(|node| node.id == call.from)
                        .is_some_and(|node| {
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
                    if let Some(action) = call.action {
                        ui.label(format!("Possible action: {}", process::action(action)));
                    }
                    ui.label(match call.scope {
                        Some(evm_abstract_protocol::ProcessControlScope::Primitive) => {
                            "Control scope: one primitive action"
                        }
                        None => "Control scope: not recorded",
                    });
                    if let Some(target) = &call.target {
                        ui.monospace(format!("Action target: {}", process::owner(target)));
                    }
                    for delivery in &call.deliveries {
                        for case in &delivery.creation {
                            process::completion(ui, case);
                        }
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

pub(in crate::app::inspector) fn node_details(
    ui: &mut Ui,
    node: &RegionalNode,
    source: SummarySourceStatus,
) {
    let frame = &node.frame;
    if let Some(key) = &frame.process {
        process::frame(ui, key);
    }
    if let Some(address) = &frame.code_address {
        ui.monospace(format!("Code: {address}"));
    }
    if let Some(hash) = &frame.code_hash {
        ui.monospace(format!("Frame identity hash: {hash}"));
    }
    if let Some(address) = &frame.storage_address {
        ui.monospace(format!("Storage namespace: {address}"));
    }
    if frame.process.is_none() {
        ui.label(format!(
            "ADDRESS: {:?} · CALLER: {:?}",
            frame.address_value, frame.caller
        ));
    }
    ui.label(format!(
        "Mode: {:?} · static: {}",
        frame.kind, frame.is_static
    ));
    if let Some(block) = frame.basic_block {
        ui.label(format!(
            "Program block: {block} · stack height: {} · jump context: {:?}",
            frame
                .stack_height
                .map_or_else(|| "Not recorded".into(), |height| height.to_string()),
            frame.context
        ));
    }
    if frame.kind == CodeKind::AbstractEnvironment
        && node
            .transfer
            .as_ref()
            .is_none_or(|transfer| transfer.process.is_none())
    {
        ui.label("Effect/environment abstraction: callbacks may be represented by effects even when the retained call list is empty.");
    }
    ui.label(format!(
        "Observed input revision: {}",
        node.input_revision.0
    ));
    ui.label(format!("Excluded input: {}", node.excluded));
    ui.label(format!("Admitted successors: {:?}", node.successors));
    if let Some(transfer) = &node.transfer {
        ui.label(process::interpretation(transfer.semantics));
        if let Some(receipt) = &transfer.process {
            if let Some(control) = &receipt.control {
                ui.label(format!(
                    "MAY control directory {} · entry {:?} · {} pure locations · {} boundary obligations",
                    control.relation, control.entry, control.through.len(), control.boundaries.len()
                ));
            }
            for allocation in &receipt.creations {
                ui.label(format!(
                    "Allocation action: {} · creator: {}",
                    process::action(allocation.action),
                    process::owner(&allocation.creator)
                ));
                ui.monospace(format!("Allocation epoch: {}", allocation.epoch));
                ui.label(format!(
                    "Creating-frame exceptions: {:?}",
                    allocation.exceptions
                ));
                for rejection in &allocation.rejections {
                    ui.label(format!(
                        "Creation result: zero · {:?} · nonce increment retained: {}",
                        rejection.reason, rejection.increments_nonce
                    ));
                    if let Some(target) = &rejection.destination {
                        ui.label(format!(
                            "Rejected destination class: {}",
                            process::owner(target)
                        ));
                    }
                }
            }
            ui.label("Conservative process rules describe possible control; instruction visits are recorded separately.");
            ui.label(format!(
                "Actions may repeat before an exit: {}",
                receipt.repeat
            ));
            ui.label(format!(
                "Represented actions: {}",
                receipt
                    .actions
                    .iter()
                    .map(|kind| process::action(*kind))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            for reason in &receipt.unresolved {
                ui.label(format!("Unexpanded: {}", process::boundary(reason)));
            }
        }
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
            "Retained calls: {:?} · recorded exits: {:?}",
            transfer.calls, transfer.outcomes
        ));
    } else {
        ui.label("Admitted; transfer pending");
    }
}
