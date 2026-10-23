//! Render actual regional receipts without synthesizing instruction/SSA evidence.
use crate::analysis::{EvmRegionalCfg, FrameCode};
use std::fmt::Write;

#[cfg(test)]
mod tests;

pub(super) fn write(
    output: &mut String,
    cfg: &EvmRegionalCfg,
    source_current: bool,
    superseded: bool,
) {
    let source_scope = if superseded {
        "historical observation"
    } else if source_current {
        "final source certified"
    } else {
        "final source uncertified"
    };
    writeln!(
        output,
        "    Regional control flow | nodes={} | calls={}",
        cfg.nodes.len(),
        cfg.calls.len()
    )
    .unwrap();
    for node in &cfg.nodes {
        let frame = &node.frame;
        writeln!(output, "      r{} | procedure={} | {:?} | code={} | storage namespace={} | static={} | input revision={} | excluded={}", node.id, node.procedure, frame.mode, frame.code_address, frame.address, frame.is_static, node.input_revision, node.excluded).unwrap();
        writeln!(
            output,
            "        ADDRESS={:?} | CALLER={:?}",
            frame.address_value, frame.caller
        )
        .unwrap();
        if frame.mode == FrameCode::Unknown {
            output.push_str("        effect/environment abstraction; callbacks can be represented by effects even with an empty retained call list\n");
        } else {
            writeln!(output, "        frame identity hash={}", frame.code_hash).unwrap();
            if matches!(frame.mode, FrameCode::Runtime | FrameCode::InitCode) {
                writeln!(
                    output,
                    "        program block={} | stack height={} | jump context={:?}",
                    frame.basic_block_index, frame.stack_height, frame.jump_history
                )
                .unwrap();
            }
        }
        writeln!(output, "        admitted successors={:?}", node.successors).unwrap();
        if let Some(transfer) = &node.transfer {
            let local_current = transfer.current
                && transfer.input_revision == node.input_revision
                && !node.excluded;
            let label = if superseded {
                "historical observation transfer"
            } else if !source_current {
                "uncertified source transfer"
            } else if local_current {
                "current transfer"
            } else {
                "historical transfer"
            };
            writeln!(output, "        {label} | record-local current={local_current} | input revision={} | complete={} | visited block={} | calls={:?} | outcomes={:?}", transfer.input_revision, transfer.complete, transfer.visited_block, transfer.calls, transfer.outcomes).unwrap();
            if let Some(process) = &transfer.process {
                writeln!(output, "        coarse {:?} process | repeat={} | actions={:?} | unresolved={:?} | no instruction evidence",
                    transfer.semantics, process.repeat, process.actions, process.unresolved).unwrap();
            }
            for edge in &transfer.successors {
                writeln!(
                    output,
                    "          {label}: r{} -> r{} | {:?}",
                    node.id, edge.to, edge.kind
                )
                .unwrap();
            }
        } else {
            output.push_str("        admitted; transfer pending\n");
        }
    }
    for call in &cfg.calls {
        writeln!(output, "      call c{} | source scope={source_scope} | from=r{} | callee procedure={} | entry nodes={:?} | input revision={}", call.id, call.from, call.callee, call.entries, call.input_revision).unwrap();
        if let Some(action) = call.action {
            writeln!(
                output,
                "        coarse action={action:?} | target={:?}",
                call.target
            )
            .unwrap();
        }
        for delivery in &call.deliveries {
            let local_current = delivery.current && delivery.input_revision == call.input_revision;
            let label = if superseded {
                "historical observation delivery"
            } else if !source_current {
                "uncertified source delivery"
            } else if local_current {
                "current delivery"
            } else {
                "historical delivery"
            };
            writeln!(
                output,
                "        {label} | {:?} | record-local current={local_current} | input revision={} | output generation={}",
                delivery.kind, delivery.input_revision, delivery.output_generation
            )
            .unwrap();
            for edge in &delivery.successors {
                writeln!(
                    output,
                    "          {label}: r{} -> r{} | {:?}",
                    call.from, edge.to, edge.kind
                )
                .unwrap();
            }
        }
    }
}
