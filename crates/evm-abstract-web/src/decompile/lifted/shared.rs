//! Inspector presentation of certified definitions and exact call occurrences.
//! Every reference comes from the model's supplied source and binding records.

use super::{Details, StageIndex, work::Work};
use evm_abstract_protocol::decompile_model::{
    EntityId, EntityRef, LiftedRenderError, SharedInterface, Stage,
};

mod prepare;
pub(in crate::decompile) use prepare::Prepared;

pub(super) fn details(
    stage: &Stage,
    index: &StageIndex,
    id: EntityId,
    work: &Work,
) -> Option<Details> {
    let mut lines = Vec::new();
    let mut references = Vec::new();
    if let Some(procedure) = index.procedure(&stage.ir, id) {
        let header = index.shared.header(id, true);
        let admitted = header.is_some();
        lines.push(format!(
            "{} shared procedure p{} · source ast{}",
            status(admitted),
            id.0,
            procedure.body.0,
        ));
        if let Some(header) = header {
            lines.push(copied(header, work));
        } else {
            lines
                .push("Source proof unavailable; original source occurrences are retained.".into());
        }
        references.push(procedure.body);
        if work.reserve(procedure.interface.record_count().saturating_mul(32)) {
            interface(&procedure.interface, &mut lines, &mut references);
        } else {
            lines.push(LiftedRenderError::Limit.to_string());
        }
        let calls = index.shared.invocations(id);
        if work.reserve(calls.len()) {
            references.extend_from_slice(calls);
        }
    } else {
        let invocation = index.invocation(&stage.ir, id)?;
        let call = index
            .shared
            .invocation(invocation.source, true)
            .filter(|(entity, _)| *entity == id);
        let admitted = call.is_some();
        lines.push(format!(
            "{} shared invocation h{} · procedure p{} · source ast{}",
            status(admitted),
            id.0,
            invocation.procedure.0,
            invocation.source.0,
        ));
        if let Some((_, call)) = call {
            lines.push(copied(call, work));
        } else {
            lines.push("Source proof unavailable; original source occurrence is retained.".into());
        }
        lines.push(format!(
            "Lexical owner {:?} · child {} · continuation {:?}",
            invocation.owner, invocation.position, invocation.continuation,
        ));
        references.extend([invocation.procedure, invocation.source]);
        references.extend(invocation.owner.into_iter().chain(invocation.continuation));
        lines.push(format!(
            "Source bindings: {} explicit replacements",
            invocation.bindings.len()
        ));
        if work.reserve(invocation.bindings.len().saturating_mul(2)) {
            for binding in &invocation.bindings {
                references.extend([binding.formal, binding.actual]);
            }
        } else {
            lines.push(LiftedRenderError::Limit.to_string());
        }
    }
    Some(Details {
        lines,
        references: references
            .into_iter()
            .map(|entity| EntityRef {
                stage: stage.id,
                entity,
            })
            .collect(),
    })
}

fn copied(text: &str, work: &Work) -> String {
    if work.reserve(text.len()) {
        text.to_owned()
    } else {
        LiftedRenderError::Limit.to_string()
    }
}

fn status(admitted: bool) -> &'static str {
    if admitted { "Certified" } else { "Unadmitted" }
}

fn interface(interface: &SharedInterface, lines: &mut Vec<String>, references: &mut Vec<EntityId>) {
    for (name, values) in [
        ("Word inputs", &interface.inputs),
        ("Word outputs", &interface.outputs),
        ("Memory inputs", &interface.memory_inputs),
        ("Memory outputs", &interface.memory_outputs),
        ("Effect inputs", &interface.effect_inputs),
        ("Effect outputs", &interface.effect_outputs),
    ] {
        lines.push(format!("{name}: {values:?}"));
        references.extend(values.iter().copied());
    }
    for continuation in &interface.continuations {
        lines.push(format!(
            "Return continuation e{} → b{}",
            continuation.edge.0, continuation.target.0
        ));
        references.extend([continuation.edge, continuation.target]);
    }
}
