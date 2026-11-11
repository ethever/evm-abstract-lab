//! Source roles come from the shared model; the Inspector never derives epochs.
use super::work::Work;
use evm_abstract_protocol::decompile_model::{EntityId, LiftedRenderIndex};

pub(in crate::decompile) fn details(
    renderer: &LiftedRenderIndex<'_>,
    operation: EntityId,
    work: &Work,
    lines: &mut Vec<String>,
    references: &mut Vec<EntityId>,
) {
    if let Some(call) = renderer.call_result(operation) {
        if !work.reserve(
            call.readers
                .len()
                .saturating_add(call.consumers.len())
                .saturating_add(3),
        ) {
            lines.push("Call result details unavailable (work limit reached)".into());
            return;
        }
        lines.push(format!(
            "Call result #{} · source o{}",
            call.ordinal, call.operation.0
        ));
        if let Some(value) = call.scalar {
            lines.push(format!("Status result remains raw word v{}", value.0));
        }
        lines.push("Returndata observations and terminal consumers retain their original source operations.".into());
        references.extend(call.source_references());
    } else if let Some(call) = renderer.returndata_source(operation) {
        lines.push(format!("Returndata source: o{}", call.0));
        references.push(call);
    }
}
