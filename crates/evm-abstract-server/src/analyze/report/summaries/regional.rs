//! Copy compact identities/receipts once; never expand numerical payloads or SSA.
use super::outcome;
use crate::analyze::{mapping, pools, process, symbolic::Pool, value};
use evm_abstract::analysis;
use evm_abstract_protocol as api;

#[cfg(test)]
mod tests;

pub(super) fn project(
    native: &analysis::EvmRegionalCfg,
    procedures: &[api::AbstractProcedure],
    export: &mut Pool<'_>,
) -> Result<api::RegionalCfg, api::ApiError> {
    export.charge(native.nodes.len().saturating_add(native.calls.len()))?;
    let mut nodes = Vec::with_capacity(native.nodes.len());
    for node in &native.nodes {
        // Fixed-size addresses/hash and fields, plus the two copied ID vectors.
        // Each variable collection is prepaid before copying, with cancellation
        // checkpoints inside receipt traversal and adjacent to publication.
        export.charge(
            32usize
                .saturating_add(node.frame.jump_history.len())
                .saturating_add(node.successors.len()),
        )?;
        let frame = &node.frame;
        let bytecode = procedures[node.procedure].semantics == api::TransferSemantics::Native
            && matches!(
                frame.mode,
                analysis::FrameCode::Runtime | analysis::FrameCode::InitCode
            );
        let transfer = node
            .transfer
            .as_ref()
            .map(|transfer| -> Result<_, api::ApiError> {
                export.charge(
                    8usize
                        .saturating_add(transfer.calls.len())
                        .saturating_add(transfer.outcomes.len()),
                )?;
                Ok(api::RegionalTransfer {
                    semantics: process::semantics(transfer.semantics),
                    process: transfer
                        .process
                        .as_ref()
                        .map(|receipt| process::transfer(receipt, export))
                        .transpose()?,
                    input_revision: api::DecimalU64(transfer.input_revision),
                    current: transfer.current,
                    complete: transfer.complete,
                    visited_block: transfer.visited_block,
                    successors: edges(&transfer.successors, export)?,
                    calls: transfer.calls.clone(),
                    outcomes: transfer.outcomes.iter().copied().map(outcome).collect(),
                })
            })
            .transpose()?;
        nodes.push(api::RegionalNode {
            id: node.id,
            procedure: node.procedure,
            frame: api::RegionalFrame {
                code_address: frame.code_address.to_string(),
                code_hash: (frame.mode != analysis::FrameCode::Unknown)
                    .then(|| frame.code_hash.to_string()),
                storage_address: frame.address.to_string(),
                address_value: value::address(frame.address_value),
                caller: value::address(frame.caller),
                kind: pools::mode(frame.mode),
                is_static: frame.is_static,
                basic_block: bytecode.then_some(frame.basic_block_index),
                stack_height: frame.stack_height,
                context: frame.jump_history.clone(),
            },
            input_revision: api::DecimalU64(node.input_revision),
            excluded: node.excluded,
            successors: node.successors.clone(),
            transfer,
        });
    }
    let mut calls = Vec::with_capacity(native.calls.len());
    for call in &native.calls {
        export.charge(8usize.saturating_add(call.entries.len()))?;
        let mut deliveries = Vec::with_capacity(call.deliveries.len());
        for delivery in &call.deliveries {
            export.charge(5)?;
            deliveries.push(api::RegionalReturn {
                kind: outcome(delivery.kind),
                input_revision: api::DecimalU64(delivery.input_revision),
                output_generation: delivery.output_generation,
                current: delivery.current,
                successors: edges(&delivery.successors, export)?,
            });
        }
        calls.push(api::RegionalCall {
            action: call.action.map(process::action),
            target: call.target.map(process::owner),
            id: call.id,
            from: call.from,
            callee: call.callee,
            entries: call.entries.clone(),
            input_revision: api::DecimalU64(call.input_revision),
            deliveries,
        });
    }
    export.charge(0)?;
    Ok(api::RegionalCfg { nodes, calls })
}

fn edges(
    native: &[analysis::EvmRegionalEdge],
    export: &mut Pool<'_>,
) -> Result<Vec<api::RegionalEdge>, api::ApiError> {
    let mut edges = Vec::with_capacity(native.len());
    for edge in native {
        export.charge(2)?;
        edges.push(api::RegionalEdge {
            to: edge.to,
            kind: mapping::edge(edge.kind),
        });
    }
    Ok(edges)
}
