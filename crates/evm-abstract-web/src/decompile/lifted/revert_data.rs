//! Inspector metadata comes from the shared data model and formatter. Raw SSA
//! references retain their failure-occurrence scope and remain navigable.
use super::work::Work;
use evm_abstract_protocol::decompile_model::{
    EntityId, LiftedRenderIndex, LiftedRevertData, LiftedRevertFormat,
};

pub(super) fn details(
    data: &LiftedRevertData,
    renderer: &LiftedRenderIndex<'_>,
    work: &Work,
    lines: &mut Vec<String>,
    references: &mut Vec<EntityId>,
) {
    let admitted = renderer.revert_data(data.id) == Some(data);
    lines.push(format!(
        "{} REVERT data h{} · b{} · o{}",
        if admitted { "Recovered" } else { "Unadmitted" },
        data.id.0,
        data.block.0,
        data.operation.0,
    ));
    if !work.reserve(data.record_count()) {
        lines.push(
            "Data inspection work limit reached; original memory slice and raw REVERT retained."
                .into(),
        );
        references.extend([data.block, data.operation]);
        return;
    }
    lines.push(format!(
        "Terminal memory m{} · offset v{} · size v{}",
        data.memory.0, data.offset.0, data.size.0,
    ));
    if let Some(hash) = data.code_hash {
        lines.push(format!("Captured runtime code hash: {hash:#x}"));
    }
    match data.decode_format(|amount| work.reserve(amount)) {
        Ok(format) => {
            lines.push(format!("Decoded format: {}", format.label()));
            if let Some(explanation) = format.panic_explanation() {
                lines.push(format!("Decoded explanation: {explanation}"));
            }
            if matches!(format, LiftedRevertFormat::Raw) {
                lines.push(
                    "Format unavailable; original memory slice and raw REVERT retained.".into(),
                );
            }
        }
        Err(error) => lines.push(format!(
            "Data format unavailable ({error}); original memory slice and raw REVERT retained."
        )),
    }
    if admitted {
        match renderer.render_revert_data(data.id, |amount| work.reserve(amount)) {
            Ok(text) => lines.push(format!("Rendered data: {text}")),
            Err(error) => lines.push(format!(
                "Data rendering unavailable ({error}); original memory slice and raw REVERT retained."
            )),
        }
    } else {
        lines.push(
            "Data source proof unavailable; original construction and raw REVERT retained.".into(),
        );
    }
    lines.push(format!(
        "Ordered byte-write witnesses: {}",
        data.writes
            .iter()
            .map(|id| format!("o{}", id.0))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    references.extend(data.source_references());
}
